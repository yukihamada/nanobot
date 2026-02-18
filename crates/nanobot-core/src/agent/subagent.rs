use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info};

use crate::config::ExecToolConfig;
use crate::provider::LlmProvider;
use crate::tool::filesystem::{ListDirTool, ReadFileTool, WriteFileTool};
use crate::tool::shell::ExecTool;
use crate::tool::web::{WebFetchTool, WebSearchTool};
use crate::tool::ToolRegistry;
use crate::types::{InboundMessage, Message};

/// Manages background subagent execution.
pub struct SubagentManager {
    provider: Arc<dyn LlmProvider>,
    workspace: PathBuf,
    model: String,
    brave_api_key: Option<String>,
    exec_config: ExecToolConfig,
    restrict_to_workspace: bool,
    inbound_tx: mpsc::Sender<InboundMessage>,
}

impl SubagentManager {
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        model: String,
        brave_api_key: Option<String>,
        exec_config: ExecToolConfig,
        restrict_to_workspace: bool,
        inbound_tx: mpsc::Sender<InboundMessage>,
    ) -> Self {
        Self {
            provider,
            workspace,
            model,
            brave_api_key,
            exec_config,
            restrict_to_workspace,
            inbound_tx,
        }
    }

    /// Spawn a subagent to execute a task in the background.
    pub async fn spawn(
        &self,
        task: &str,
        label: Option<&str>,
        origin_channel: &str,
        origin_chat_id: &str,
    ) {
        let task_id = uuid::Uuid::new_v4().to_string()[..8].to_string();
        let display_label = label
            .unwrap_or_else(|| {
                if task.len() > 30 {
                    &task[..30]
                } else {
                    task
                }
            })
            .to_string();

        info!("Spawned subagent [{}]: {}", task_id, display_label);

        let provider = self.provider.clone();
        let workspace = self.workspace.clone();
        let model = self.model.clone();
        let brave_api_key = self.brave_api_key.clone();
        let exec_config = self.exec_config.clone();
        let restrict = self.restrict_to_workspace;
        let inbound_tx = self.inbound_tx.clone();
        let task_str = task.to_string();
        let label_str = display_label.clone();
        let origin_ch = origin_channel.to_string();
        let origin_id = origin_chat_id.to_string();

        tokio::spawn(async move {
            let result = run_subagent(
                &task_id,
                &task_str,
                &label_str,
                provider,
                &workspace,
                &model,
                brave_api_key,
                exec_config,
                restrict,
            )
            .await;

            let (status, result_text) = match result {
                Ok(text) => ("ok", text),
                Err(e) => ("error", format!("Error: {e}")),
            };

            // Announce result
            let announce_content = format!(
                "[Subagent '{}' {}]\n\nTask: {}\n\nResult:\n{}\n\n\
                Summarize this naturally for the user. Keep it brief (1-2 sentences). \
                Do not mention technical details like \"subagent\" or task IDs.",
                label_str,
                if status == "ok" {
                    "completed successfully"
                } else {
                    "failed"
                },
                task_str,
                result_text,
            );

            let msg = InboundMessage::new(
                "system",
                "subagent",
                format!("{origin_ch}:{origin_id}"),
                &announce_content,
            );

            if let Err(e) = inbound_tx.send(msg).await {
                error!("Failed to announce subagent result: {}", e);
            }
        });
    }
}

async fn run_subagent(
    task_id: &str,
    task: &str,
    label: &str,
    provider: Arc<dyn LlmProvider>,
    workspace: &Path,
    model: &str,
    brave_api_key: Option<String>,
    exec_config: ExecToolConfig,
    restrict_to_workspace: bool,
) -> anyhow::Result<String> {
    info!("Subagent [{}] starting task: {}", task_id, label);

    let tools = Arc::new(ToolRegistry::new());
    let allowed_dir = if restrict_to_workspace {
        Some(workspace.to_path_buf())
    } else {
        None
    };

    tools.register(Arc::new(ReadFileTool::new(allowed_dir.clone())));
    tools.register(Arc::new(WriteFileTool::new(allowed_dir.clone())));
    tools.register(Arc::new(ListDirTool::new(allowed_dir)));
    tools.register(Arc::new(ExecTool::new(
        workspace.display().to_string(),
        exec_config.timeout,
        restrict_to_workspace,
    )));
    tools.register(Arc::new(WebSearchTool::new(brave_api_key, 5)));
    tools.register(Arc::new(WebFetchTool::new(50000)));

    let system_prompt = format!(
        r#"# Subagent

You are a subagent spawned by the main agent to complete a specific task.

## Your Task
{task}

## Rules
1. Stay focused - complete only the assigned task, nothing else
2. Your final response will be reported back to the main agent
3. Do not initiate conversations or take on side tasks
4. Be concise but informative in your findings

## What You Can Do
- Read and write files in the workspace
- Execute shell commands
- Search the web and fetch web pages
- Complete the task thoroughly

## What You Cannot Do
- Send messages directly to users (no message tool available)
- Spawn other subagents
- Access the main agent's conversation history

## Workspace
Your workspace is at: {workspace}

When you have completed the task, provide a clear summary of your findings or actions."#,
        workspace = workspace.display()
    );

    let mut messages = vec![
        Message::system(system_prompt),
        Message::user(task),
    ];

    let max_iterations = 15;
    for iteration in 0..max_iterations {
        debug!("Subagent [{}] iteration {}", task_id, iteration + 1);

        let tools_defs = tools.get_definitions();
        let response = provider
            .chat(
                &messages,
                if tools_defs.is_empty() {
                    None
                } else {
                    Some(&tools_defs)
                },
                model,
                8192,
                0.7,
            )
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        if response.has_tool_calls() {
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

            for tc in &response.tool_calls {
                debug!(
                    "Subagent [{}] executing: {}",
                    task_id, tc.name
                );
                let result = tools.execute(&tc.name, tc.arguments.clone()).await;
                messages.push(Message::tool_result(&tc.id, &tc.name, &result));
            }
        } else {
            info!("Subagent [{}] completed successfully", task_id);
            return Ok(response
                .content
                .unwrap_or_else(|| "Task completed but no final response was generated.".to_string()));
        }
    }

    Ok("Task completed but no final response was generated.".to_string())
}
