use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::config::ZaloConfig;
use crate::types::{InboundMessage, OutboundMessage};

use super::{is_allowed, Channel};

const ZALO_OA_API_BASE: &str = "https://openapi.zalo.me/v3.0/oa";

/// Zalo OA channel using webhook or long-polling.
pub struct ZaloChannel {
    config: ZaloConfig,
    inbound_tx: mpsc::Sender<InboundMessage>,
    running: bool,
    client: reqwest::Client,
}

impl ZaloChannel {
    pub fn new(config: ZaloConfig, inbound_tx: mpsc::Sender<InboundMessage>) -> Self {
        Self {
            config,
            inbound_tx,
            running: false,
            client: reqwest::Client::new(),
        }
    }

    /// Process an incoming Zalo webhook event.
    pub async fn process_event(&self, event: &ZaloEvent) {
        if event.event_name != "user_send_text" {
            debug!("Ignoring Zalo event: {}", event.event_name);
            return;
        }

        let sender_id = event.sender.as_ref().map(|s| s.id.as_str()).unwrap_or("");
        let content = event
            .message
            .as_ref()
            .and_then(|m| m.text.as_deref())
            .unwrap_or("");

        if sender_id.is_empty() || content.is_empty() {
            return;
        }

        if !is_allowed(sender_id, &self.config.allow_from) {
            warn!("Zalo message from unauthorized user: {}", sender_id);
            return;
        }

        info!("Zalo message from {}: {}", sender_id, content);

        let msg = InboundMessage::new("zalo", sender_id, sender_id, content);
        if let Err(e) = self.inbound_tx.send(msg).await {
            error!("Failed to send Zalo message to bus: {}", e);
        }
    }

    /// Send a customer-service message via Zalo OA API.
    async fn send_cs_message(&self, recipient_id: &str, text: &str) -> anyhow::Result<()> {
        let url = format!("{}/message/cs", ZALO_OA_API_BASE);

        let body = json!({
            "recipient": {
                "user_id": recipient_id,
            },
            "message": {
                "text": text,
            },
        });

        let resp = self
            .client
            .post(&url)
            .header("access_token", &self.config.bot_token)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("Zalo send error: {} {}", status, text));
        }

        let result: serde_json::Value = resp.json().await.unwrap_or_default();
        if result.get("error").and_then(|v| v.as_i64()).unwrap_or(0) != 0 {
            let err_msg = result
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            return Err(anyhow::anyhow!("Zalo API error: {}", err_msg));
        }

        Ok(())
    }

    /// Parse a Zalo webhook event from JSON body.
    pub fn parse_event(body: &str) -> Result<ZaloEvent, serde_json::Error> {
        serde_json::from_str(body)
    }
}

#[async_trait]
impl Channel for ZaloChannel {
    fn name(&self) -> &str {
        "zalo"
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        if self.config.bot_token.is_empty() {
            return Err(anyhow::anyhow!("Zalo bot token not configured"));
        }

        // Zalo uses webhooks, no persistent connection needed.
        info!("Zalo channel started (webhook mode)");
        self.running = true;
        Ok(())
    }

    async fn stop(&mut self) -> anyhow::Result<()> {
        self.running = false;
        info!("Zalo channel stopped");
        Ok(())
    }

    async fn send(&self, msg: &OutboundMessage) -> anyhow::Result<()> {
        self.send_cs_message(&msg.chat_id, &msg.content).await
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

// ====== Zalo Types ======

#[derive(Debug, Clone, Deserialize)]
pub struct ZaloEvent {
    pub event_name: String,
    pub app_id: Option<String>,
    pub sender: Option<ZaloSender>,
    pub recipient: Option<ZaloRecipient>,
    pub message: Option<ZaloMessage>,
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ZaloSender {
    pub id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ZaloRecipient {
    pub id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ZaloMessage {
    pub msg_id: Option<String>,
    pub text: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_name() {
        let (tx, _rx) = mpsc::channel(1);
        let ch = ZaloChannel::new(ZaloConfig::default(), tx);
        assert_eq!(ch.name(), "zalo");
    }

    #[test]
    fn test_parse_event_text_message() {
        let body = r#"{
            "event_name": "user_send_text",
            "app_id": "app123",
            "sender": {
                "id": "user456"
            },
            "recipient": {
                "id": "oa789"
            },
            "message": {
                "msg_id": "msg001",
                "text": "Hello from Zalo!"
            },
            "timestamp": "1625000000000"
        }"#;

        let event = ZaloChannel::parse_event(body).unwrap();
        assert_eq!(event.event_name, "user_send_text");
        assert_eq!(event.sender.as_ref().unwrap().id, "user456");
        assert_eq!(
            event.message.as_ref().unwrap().text.as_deref(),
            Some("Hello from Zalo!")
        );
    }

    #[test]
    fn test_parse_event_follow() {
        let body = r#"{
            "event_name": "follow",
            "sender": {
                "id": "user999"
            },
            "recipient": {
                "id": "oa123"
            }
        }"#;

        let event = ZaloChannel::parse_event(body).unwrap();
        assert_eq!(event.event_name, "follow");
        assert!(event.message.is_none());
    }

    #[test]
    fn test_parse_event_with_msg_id() {
        let body = r#"{
            "event_name": "user_send_text",
            "sender": {
                "id": "sender1"
            },
            "message": {
                "msg_id": "msg-id-123",
                "text": "Test message"
            }
        }"#;

        let event = ZaloChannel::parse_event(body).unwrap();
        assert_eq!(
            event.message.as_ref().unwrap().msg_id.as_deref(),
            Some("msg-id-123")
        );
    }
}
