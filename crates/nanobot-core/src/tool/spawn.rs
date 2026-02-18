use async_trait::async_trait;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::Tool;

/// Callback type for spawning a subagent.
pub type SpawnCallback =
    Arc<dyn Fn(String, Option<String>, String, String) -> tokio::task::JoinHandle<()> + Send + Sync>;

/// Tool to spawn a subagent for background task execution.
pub struct SpawnTool {
    spawn_fn: SpawnCallback,
    context: Arc<Mutex<(String, String)>>, // (channel, chat_id)
}

impl SpawnTool {
    pub fn new(spawn_fn: SpawnCallback) -> Self {
        Self {
            spawn_fn,
            context: Arc::new(Mutex::new(("cli".to_string(), "direct".to_string()))),
        }
    }

    pub async fn set_context(&self, channel: &str, chat_id: &str) {
        let mut ctx = self.context.lock().await;
        *ctx = (channel.to_string(), chat_id.to_string());
    }
}

#[async_trait]
impl Tool for SpawnTool {
    fn name(&self) -> &str {
        "spawn"
    }

    fn description(&self) -> &str {
        "Spawn a subagent to handle a task in the background. Use this for complex or time-consuming tasks that can run independently. The subagent will complete the task and report back when done."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "The task for the subagent to complete"
                },
                "label": {
                    "type": "string",
                    "description": "Optional short label for the task (for display)"
                }
            },
            "required": ["task"]
        })
    }

    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let task = match params.get("task").and_then(|v| v.as_str()) {
            Some(t) => t.to_string(),
            None => return "Error: 'task' parameter is required".to_string(),
        };
        let label = params.get("label").and_then(|v| v.as_str()).map(|s| s.to_string());

        let ctx = self.context.lock().await;
        let channel = ctx.0.clone();
        let chat_id = ctx.1.clone();
        drop(ctx);

        let display_label = label
            .as_deref()
            .unwrap_or_else(|| {
                if task.len() > 30 {
                    &task[..30]
                } else {
                    &task
                }
            })
            .to_string();

        (self.spawn_fn)(task, label.clone(), channel, chat_id);

        format!(
            "Subagent [{display_label}] started. I'll notify you when it completes."
        )
    }
}
