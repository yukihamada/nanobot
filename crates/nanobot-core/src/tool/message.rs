use async_trait::async_trait;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

use crate::types::OutboundMessage;
use super::Tool;

/// Tool to send messages to users on chat channels.
pub struct MessageTool {
    outbound_tx: mpsc::Sender<OutboundMessage>,
    context: Arc<Mutex<(String, String)>>, // (channel, chat_id)
}

impl MessageTool {
    pub fn new(outbound_tx: mpsc::Sender<OutboundMessage>) -> Self {
        Self {
            outbound_tx,
            context: Arc::new(Mutex::new((String::new(), String::new()))),
        }
    }

    pub async fn set_context(&self, channel: &str, chat_id: &str) {
        let mut ctx = self.context.lock().await;
        *ctx = (channel.to_string(), chat_id.to_string());
    }
}

#[async_trait]
impl Tool for MessageTool {
    fn name(&self) -> &str {
        "message"
    }

    fn description(&self) -> &str {
        "Send a message to the user. Use this when you want to communicate something."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "The message content to send"
                },
                "channel": {
                    "type": "string",
                    "description": "Optional: target channel (telegram, discord, etc.)"
                },
                "chat_id": {
                    "type": "string",
                    "description": "Optional: target chat/user ID"
                }
            },
            "required": ["content"]
        })
    }

    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let content = match params.get("content").and_then(|v| v.as_str()) {
            Some(c) => c.to_string(),
            None => return "Error: 'content' parameter is required".to_string(),
        };

        let ctx = self.context.lock().await;
        let channel = params
            .get("channel")
            .and_then(|v| v.as_str())
            .unwrap_or(&ctx.0)
            .to_string();
        let chat_id = params
            .get("chat_id")
            .and_then(|v| v.as_str())
            .unwrap_or(&ctx.1)
            .to_string();
        drop(ctx);

        if channel.is_empty() || chat_id.is_empty() {
            return "Error: No target channel/chat specified".to_string();
        }

        let msg = OutboundMessage::new(&channel, &chat_id, &content);
        match self.outbound_tx.send(msg).await {
            Ok(_) => format!("Message sent to {}:{}", channel, chat_id),
            Err(e) => format!("Error sending message: {}", e),
        }
    }
}
