use async_trait::async_trait;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::Tool;
use crate::service::cron::{CronSchedule, CronService};

/// Tool to schedule reminders and recurring tasks.
pub struct CronTool {
    cron_service: Arc<Mutex<CronService>>,
    context: Arc<Mutex<(String, String)>>, // (channel, chat_id)
}

impl CronTool {
    pub fn new(cron_service: Arc<Mutex<CronService>>) -> Self {
        Self {
            cron_service,
            context: Arc::new(Mutex::new((String::new(), String::new()))),
        }
    }

    pub async fn set_context(&self, channel: &str, chat_id: &str) {
        let mut ctx = self.context.lock().await;
        *ctx = (channel.to_string(), chat_id.to_string());
    }
}

#[async_trait]
impl Tool for CronTool {
    fn name(&self) -> &str {
        "cron"
    }

    fn description(&self) -> &str {
        "Schedule reminders and recurring tasks. Actions: add, list, remove."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["add", "list", "remove"],
                    "description": "Action to perform"
                },
                "message": {
                    "type": "string",
                    "description": "Reminder message (for add)"
                },
                "every_seconds": {
                    "type": "integer",
                    "description": "Interval in seconds (for recurring tasks)"
                },
                "cron_expr": {
                    "type": "string",
                    "description": "Cron expression like '0 9 * * *' (for scheduled tasks)"
                },
                "job_id": {
                    "type": "string",
                    "description": "Job ID (for remove)"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let action = match params.get("action").and_then(|v| v.as_str()) {
            Some(a) => a,
            None => return "Error: 'action' parameter is required".to_string(),
        };

        match action {
            "add" => {
                let message = params
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if message.is_empty() {
                    return "Error: message is required for add".to_string();
                }

                let ctx = self.context.lock().await;
                if ctx.0.is_empty() || ctx.1.is_empty() {
                    return "Error: no session context (channel/chat_id)".to_string();
                }
                let channel = ctx.0.clone();
                let chat_id = ctx.1.clone();
                drop(ctx);

                let schedule = if let Some(secs) = params.get("every_seconds").and_then(|v| v.as_u64()) {
                    CronSchedule::Every { every_ms: secs * 1000 }
                } else if let Some(expr) = params.get("cron_expr").and_then(|v| v.as_str()) {
                    CronSchedule::Cron {
                        expr: expr.to_string(),
                        tz: None,
                    }
                } else {
                    return "Error: either every_seconds or cron_expr is required".to_string();
                };

                let mut cron = self.cron_service.lock().await;
                let job = cron.add_job(
                    &message[..message.len().min(30)],
                    schedule,
                    message,
                    true,
                    Some(&channel),
                    Some(&chat_id),
                );
                format!("Created job '{}' (id: {})", job.name, job.id)
            }
            "list" => {
                let cron = self.cron_service.lock().await;
                let jobs = cron.list_jobs(false);
                if jobs.is_empty() {
                    "No scheduled jobs.".to_string()
                } else {
                    let lines: Vec<String> = jobs
                        .iter()
                        .map(|j| format!("- {} (id: {}, {})", j.name, j.id, j.schedule.kind_str()))
                        .collect();
                    format!("Scheduled jobs:\n{}", lines.join("\n"))
                }
            }
            "remove" => {
                let job_id = match params.get("job_id").and_then(|v| v.as_str()) {
                    Some(id) => id,
                    None => return "Error: job_id is required for remove".to_string(),
                };
                let mut cron = self.cron_service.lock().await;
                if cron.remove_job(job_id) {
                    format!("Removed job {}", job_id)
                } else {
                    format!("Job {} not found", job_id)
                }
            }
            _ => format!("Unknown action: {}", action),
        }
    }
}
