pub mod context;
pub mod ooda;
pub mod personality;
pub mod subagent;

use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info};

use crate::bus::MessageBus;
use crate::config::ExecToolConfig;
use crate::provider::LlmProvider;
use crate::session::file_store::FileSessionStore;
use crate::session::store::SessionStore;
use crate::tool::filesystem::{EditFileTool, ListDirTool, ReadFileTool, WriteFileTool};
use crate::tool::message::MessageTool;
use crate::tool::shell::ExecTool;
use crate::tool::spawn::{SpawnCallback, SpawnTool};
use crate::tool::web::{WebFetchTool, WebSearchTool};
use crate::tool::ToolRegistry;
use crate::types::{InboundMessage, Message, OutboundMessage};

use self::context::ContextBuilder;
use self::subagent::SubagentManager;

/// The agent loop is the core processing engine.
#[allow(dead_code)]
pub struct AgentLoop {
    provider: Arc<dyn LlmProvider>,
    workspace: PathBuf,
    model: String,
    max_iterations: u32,
    context: ContextBuilder,
    sessions: Box<dyn SessionStore>,
    tools: Arc<ToolRegistry>,
    message_tool: Arc<MessageTool>,
    inbound_rx: mpsc::Receiver<InboundMessage>,
    outbound_tx: mpsc::Sender<OutboundMessage>,
    inbound_tx: mpsc::Sender<InboundMessage>,
}

impl AgentLoop {
    pub fn new(
        bus: MessageBus,
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        model: Option<String>,
        max_iterations: u32,
        brave_api_key: Option<String>,
        exec_config: ExecToolConfig,
        restrict_to_workspace: bool,
        subagent_manager: Option<Arc<SubagentManager>>,
    ) -> Self {
        let model = model.unwrap_or_else(|| provider.default_model().to_string());
        let context = ContextBuilder::new(&workspace);
        let sessions: Box<dyn SessionStore> = Box::new(FileSessionStore::new(&workspace));
        let tools = Arc::new(ToolRegistry::new());

        let inbound_tx = bus.inbound_sender();
        let outbound_tx = bus.outbound_sender();

        // Register default tools
        let allowed_dir = if restrict_to_workspace {
            Some(workspace.clone())
        } else {
            None
        };

        tools.register(Arc::new(ReadFileTool::new(allowed_dir.clone())));
        tools.register(Arc::new(WriteFileTool::new(allowed_dir.clone())));
        tools.register(Arc::new(EditFileTool::new(allowed_dir.clone())));
        tools.register(Arc::new(ListDirTool::new(allowed_dir)));

        tools.register(Arc::new(ExecTool::new(
            workspace.display().to_string(),
            exec_config.timeout,
            restrict_to_workspace,
        )));

        tools.register(Arc::new(WebSearchTool::new(brave_api_key, 5)));
        tools.register(Arc::new(WebFetchTool::new(50000)));

        let message_tool = Arc::new(MessageTool::new(outbound_tx.clone()));
        tools.register(message_tool.clone());

        // Spawn tool
        if let Some(mgr) = subagent_manager {
            let spawn_fn: SpawnCallback = {
                let mgr = mgr.clone();
                Arc::new(move |task, label, channel, chat_id| {
                    let mgr = mgr.clone();
                    tokio::spawn(async move {
                        mgr.spawn(&task, label.as_deref(), &channel, &chat_id)
                            .await;
                    })
                })
            };
            tools.register(Arc::new(SpawnTool::new(spawn_fn)));
        }

        let (_inbound_tx, _outbound_tx) = (bus.inbound_sender(), bus.outbound_sender());

        Self {
            provider,
            workspace,
            model,
            max_iterations,
            context,
            sessions,
            tools,
            message_tool,
            inbound_rx: tokio::sync::mpsc::channel(1).1, // placeholder, set in run_with_receiver
            outbound_tx,
            inbound_tx,
        }
    }

    /// Run the agent loop with an inbound receiver.
    pub async fn run(mut self, mut inbound_rx: mpsc::Receiver<InboundMessage>) {
        info!("Agent loop started");

        while let Some(msg) = inbound_rx.recv().await {
            match self.process_message(&msg).await {
                Ok(Some(response)) => {
                    if let Err(e) = self.outbound_tx.send(response).await {
                        error!("Failed to send outbound message: {}", e);
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    error!("Error processing message: {}", e);
                    let error_msg = OutboundMessage::new(
                        &msg.channel,
                        &msg.chat_id,
                        format!("Sorry, I encountered an error: {e}"),
                    );
                    self.outbound_tx.send(error_msg).await.ok();
                }
            }
        }

        info!("Agent loop stopped");
    }

    /// Process a single inbound message.
    async fn process_message(
        &mut self,
        msg: &InboundMessage,
    ) -> anyhow::Result<Option<OutboundMessage>> {
        // Handle system messages (subagent announces)
        if msg.channel == "system" {
            return self.process_system_message(msg).await;
        }

        info!(
            "Processing message from {}:{}",
            msg.channel, msg.sender_id
        );

        let session_key = msg.session_key();

        // Update tool contexts
        self.message_tool.set_context(&msg.channel, &msg.chat_id).await;

        // Build initial messages
        let session = self.sessions.get_or_create(&session_key);
        let history = session.get_history(50);
        let messages = self.context.build_messages(
            &history,
            &msg.content,
            if msg.media.is_empty() {
                None
            } else {
                Some(&msg.media)
            },
            Some(&msg.channel),
            Some(&msg.chat_id),
        );

        // Agent loop
        let final_content = self
            .run_agent_loop(messages)
            .await?;

        let final_content = final_content
            .unwrap_or_else(|| "I've completed processing but have no response to give.".to_string());

        // Save to session
        {
            let session = self.sessions.get_or_create(&session_key);
            session.add_message("user", &msg.content);
            session.add_message("assistant", &final_content);
        }
        self.sessions.save_by_key(&session_key);

        Ok(Some(OutboundMessage::new(
            &msg.channel,
            &msg.chat_id,
            &final_content,
        )))
    }

    /// Process a system message (e.g., subagent announce).
    async fn process_system_message(
        &mut self,
        msg: &InboundMessage,
    ) -> anyhow::Result<Option<OutboundMessage>> {
        info!("Processing system message from {}", msg.sender_id);

        let (origin_channel, origin_chat_id) = if let Some((ch, id)) = msg.chat_id.split_once(':') {
            (ch.to_string(), id.to_string())
        } else {
            ("cli".to_string(), msg.chat_id.clone())
        };

        let session_key = format!("{origin_channel}:{origin_chat_id}");
        self.message_tool
            .set_context(&origin_channel, &origin_chat_id)
            .await;

        let session = self.sessions.get_or_create(&session_key);
        let history = session.get_history(50);
        let messages = self.context.build_messages(
            &history,
            &msg.content,
            None,
            Some(&origin_channel),
            Some(&origin_chat_id),
        );

        let final_content = self.run_agent_loop(messages).await?.unwrap_or_else(|| {
            "Background task completed.".to_string()
        });

        let session = self.sessions.get_or_create(&session_key);
        session.add_message(
            "user",
            &format!("[System: {}] {}", msg.sender_id, msg.content),
        );
        session.add_message("assistant", &final_content);

        Ok(Some(OutboundMessage::new(
            &origin_channel,
            &origin_chat_id,
            &final_content,
        )))
    }

    /// Run the LLM -> tool -> loop cycle.
    async fn run_agent_loop(
        &self,
        mut messages: Vec<Message>,
    ) -> anyhow::Result<Option<String>> {
        for iteration in 0..self.max_iterations {
            debug!("Agent loop iteration {}", iteration + 1);

            let tools_defs = self.tools.get_definitions();
            let response = self
                .provider
                .chat(
                    &messages,
                    if tools_defs.is_empty() {
                        None
                    } else {
                        Some(&tools_defs)
                    },
                    &self.model,
                    8192,
                    0.7,
                )
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            if response.has_tool_calls() {
                // Build tool_calls JSON for message history
                let tool_call_dicts: Vec<serde_json::Value> = response
                    .tool_calls
                    .iter()
                    .map(|tc| {
                        json!({
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": tc.name,
                                "arguments": serde_json::to_string(&tc.arguments).unwrap_or_else(|_| "{}".to_string()),
                            }
                        })
                    })
                    .collect();

                messages.push(Message::assistant_with_tool_calls(
                    response.content.clone(),
                    tool_call_dicts,
                ));

                // Execute tools (concurrently if multiple)
                if response.tool_calls.len() == 1 {
                    let tc = &response.tool_calls[0];
                    let args_preview = Self::format_tool_args(&tc.arguments);
                    info!("🔧 Executing: {}({})", tc.name, args_preview);

                    let start = std::time::Instant::now();
                    let result = self.tools.execute(&tc.name, tc.arguments.clone()).await;
                    let elapsed = start.elapsed();

                    info!("✅ {} completed in {:.2}s", tc.name, elapsed.as_secs_f64());
                    messages.push(Message::tool_result(&tc.id, &tc.name, &result));
                } else {
                    // Parallel execution with join_all
                    let tool_names: Vec<String> = response.tool_calls.iter()
                        .map(|tc| format!("{}({})", tc.name, Self::format_tool_args(&tc.arguments)))
                        .collect();
                    info!("🔧 Executing {} tools in parallel: {}",
                        response.tool_calls.len(),
                        tool_names.join(", ")
                    );

                    let start = std::time::Instant::now();
                    let futures: Vec<_> = response
                        .tool_calls
                        .iter()
                        .map(|tc| {
                            let tools = self.tools.clone();
                            let name = tc.name.clone();
                            let args = tc.arguments.clone();
                            let id = tc.id.clone();
                            async move {
                                let result = tools.execute(&name, args).await;
                                (id, name, result)
                            }
                        })
                        .collect();

                    let results = futures::future::join_all(futures).await;
                    let elapsed = start.elapsed();
                    info!("✅ All tools completed in {:.2}s", elapsed.as_secs_f64());

                    for (id, name, result) in results {
                        messages.push(Message::tool_result(&id, &name, &result));
                    }
                }
            } else {
                // No tool calls, we're done
                return Ok(response.content);
            }
        }

        Ok(None)
    }

    /// Format tool arguments for logging (abbreviated to avoid clutter).
    fn format_tool_args(args: &HashMap<String, serde_json::Value>) -> String {
        if args.is_empty() {
            return String::new();
        }

        let mut parts = Vec::new();
        for (key, value) in args.iter().take(3) {
            let value_str = match value {
                serde_json::Value::String(s) => {
                    if s.len() > 50 {
                        format!("\"{}...\"", &s[..50])
                    } else {
                        format!("\"{}\"", s)
                    }
                }
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                serde_json::Value::Array(a) => format!("[{} items]", a.len()),
                serde_json::Value::Object(o) => format!("{{{} fields}}", o.len()),
                serde_json::Value::Null => "null".to_string(),
            };
            parts.push(format!("{}={}", key, value_str));
        }

        if args.len() > 3 {
            parts.push(format!("...+{} more", args.len() - 3));
        }

        parts.join(", ")
    }

    /// Process a message directly (for CLI or cron usage).
    pub async fn process_direct(
        &mut self,
        content: &str,
        _session_key: &str,
        channel: &str,
        chat_id: &str,
    ) -> anyhow::Result<String> {
        let msg = InboundMessage::new(channel, "user", chat_id, content);
        let response = self.process_message(&msg).await?;
        Ok(response.map(|r| r.content).unwrap_or_default())
    }

    /// Self-correction loop: verify code with linter/tests, auto-fix if errors found.
    /// Returns (success: bool, final_output: String, iterations: u32)
    pub async fn self_correct(
        &mut self,
        language: &str,
        path: &str,
        max_iterations: u32,
    ) -> anyhow::Result<(bool, String, u32)> {
        info!("🔄 Starting self-correction loop (max {} iterations)", max_iterations);

        let mut iteration = 0;
        let mut last_output = String::new();

        while iteration < max_iterations {
            iteration += 1;
            info!("🔄 Self-correction iteration {}/{}", iteration, max_iterations);

            // Step 1: Run linter
            info!("🔍 Running linter...");
            let mut lint_args = HashMap::new();
            lint_args.insert("language".to_string(), serde_json::json!(language));
            lint_args.insert("path".to_string(), serde_json::json!(path));
            let lint_result = self.tools.execute("run_linter", lint_args).await;

            // Step 2: Run tests
            info!("🧪 Running tests...");
            let mut test_args = HashMap::new();
            test_args.insert("language".to_string(), serde_json::json!(language));
            test_args.insert("path".to_string(), serde_json::json!(path));
            let test_result = self.tools.execute("run_tests", test_args).await;

            last_output = format!("Linter:\n{}\n\nTests:\n{}", lint_result, test_result);

            // Step 3: Check if passed
            let lint_passed = lint_result.contains("✅") || lint_result.contains("no issues");
            let tests_passed = test_result.contains("✅") || test_result.contains("passed");

            if lint_passed && tests_passed {
                info!("✅ All checks passed!");
                return Ok((true, last_output, iteration));
            }

            // Step 4: If last iteration, return failure
            if iteration >= max_iterations {
                info!("❌ Max iterations reached, checks still failing");
                return Ok((false, last_output, iteration));
            }

            // Step 5: Ask LLM to fix errors
            info!("🤖 Asking LLM to fix errors...");
            let fix_prompt = format!(
                "The following linter and test errors were found:\n\n{}\n\n\
                Please analyze these errors and fix the code. Use file operations tools \
                (read_file, write_file, edit_file) to make the necessary changes. \
                Focus on fixing the actual errors, not refactoring or adding features.",
                last_output
            );

            let msg = InboundMessage::new("cli", "system", "self-correction", &fix_prompt);
            match self.process_message(&msg).await? {
                Some(response) => {
                    info!("🔧 LLM applied fixes:\n{}", response.content);
                }
                None => {
                    error!("❌ LLM failed to respond");
                    return Ok((false, "LLM failed to respond".to_string(), iteration));
                }
            }

            // Loop continues to re-check
        }

        Ok((false, last_output, iteration))
    }

    /// Verify code quality: run linter + tests, return detailed report.
    pub async fn verify_code(
        &self,
        language: &str,
        path: &str,
    ) -> anyhow::Result<CodeVerificationReport> {
        info!("🔍 Verifying code quality for {} at {}", language, path);

        // Run linter
        let mut lint_args = HashMap::new();
        lint_args.insert("language".to_string(), serde_json::json!(language));
        lint_args.insert("path".to_string(), serde_json::json!(path));
        let lint_result = self.tools.execute("run_linter", lint_args).await;

        // Run tests
        let mut test_args = HashMap::new();
        test_args.insert("language".to_string(), serde_json::json!(language));
        test_args.insert("path".to_string(), serde_json::json!(path));
        let test_result = self.tools.execute("run_tests", test_args).await;

        let lint_passed = lint_result.contains("✅") || lint_result.contains("no issues");
        let tests_passed = test_result.contains("✅") || test_result.contains("passed");

        Ok(CodeVerificationReport {
            lint_passed,
            lint_output: lint_result,
            tests_passed,
            test_output: test_result,
            overall_passed: lint_passed && tests_passed,
        })
    }
}

/// Code verification report from linter + tests
#[derive(Debug, Clone)]
pub struct CodeVerificationReport {
    pub lint_passed: bool,
    pub lint_output: String,
    pub tests_passed: bool,
    pub test_output: String,
    pub overall_passed: bool,
}
