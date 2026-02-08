use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::config::GoogleChatConfig;
use crate::types::{InboundMessage, OutboundMessage};

use super::{is_allowed, Channel};

const GOOGLE_CHAT_API_BASE: &str = "https://chat.googleapis.com/v1";

/// Google Chat channel using webhooks for receiving and REST API for sending.
pub struct GoogleChatChannel {
    config: GoogleChatConfig,
    inbound_tx: mpsc::Sender<InboundMessage>,
    running: bool,
    client: reqwest::Client,
}

impl GoogleChatChannel {
    pub fn new(config: GoogleChatConfig, inbound_tx: mpsc::Sender<InboundMessage>) -> Self {
        Self {
            config,
            inbound_tx,
            running: false,
            client: reqwest::Client::new(),
        }
    }

    /// Process an incoming Google Chat webhook event.
    pub async fn process_event(&self, event: &GoogleChatEvent) {
        if event.event_type != "MESSAGE" {
            debug!("Ignoring Google Chat event type: {}", event.event_type);
            return;
        }

        let message = match &event.message {
            Some(m) => m,
            None => return,
        };

        let sender_id = event
            .user
            .as_ref()
            .map(|u| u.name.as_str())
            .unwrap_or("")
            .to_string();

        let space_name = event
            .space
            .as_ref()
            .map(|s| s.name.as_str())
            .unwrap_or("")
            .to_string();

        let content = message.text.as_deref().unwrap_or("").to_string();

        if sender_id.is_empty() || space_name.is_empty() || content.is_empty() {
            return;
        }

        if !is_allowed(&sender_id, &self.config.allow_from) {
            warn!(
                "Google Chat message from unauthorized user: {}",
                sender_id
            );
            return;
        }

        info!("Google Chat message from {}: {}", sender_id, content);

        let msg = InboundMessage::new("google_chat", &sender_id, &space_name, &content);
        if let Err(e) = self.inbound_tx.send(msg).await {
            error!("Failed to send Google Chat message to bus: {}", e);
        }
    }

    /// Send a message to a Google Chat space.
    async fn send_to_space(
        &self,
        space: &str,
        text: &str,
        access_token: &str,
    ) -> anyhow::Result<()> {
        let url = format!("{}/{}/messages", GOOGLE_CHAT_API_BASE, space);

        let body = json!({
            "text": text,
        });

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", access_token))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "Google Chat send error: {} {}",
                status,
                text
            ));
        }

        Ok(())
    }

    /// Parse a Google Chat webhook event from JSON body.
    pub fn parse_event(body: &str) -> Result<GoogleChatEvent, serde_json::Error> {
        serde_json::from_str(body)
    }
}

#[async_trait]
impl Channel for GoogleChatChannel {
    fn name(&self) -> &str {
        "google_chat"
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        // Google Chat uses webhooks, no persistent connection needed.
        info!("Google Chat channel started (webhook mode)");
        self.running = true;
        Ok(())
    }

    async fn stop(&mut self) -> anyhow::Result<()> {
        self.running = false;
        info!("Google Chat channel stopped");
        Ok(())
    }

    async fn send(&self, msg: &OutboundMessage) -> anyhow::Result<()> {
        // Access token should be provided in metadata (obtained from service account)
        let access_token = msg
            .metadata
            .get("access_token")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if access_token.is_empty() {
            return Err(anyhow::anyhow!(
                "Google Chat: no access_token in message metadata"
            ));
        }

        self.send_to_space(&msg.chat_id, &msg.content, access_token)
            .await
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

// ====== Google Chat Types ======

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleChatEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub event_time: Option<String>,
    pub space: Option<GoogleChatSpace>,
    pub message: Option<GoogleChatMessage>,
    pub user: Option<GoogleChatUser>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleChatSpace {
    pub name: String,
    pub display_name: Option<String>,
    #[serde(rename = "type")]
    pub space_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleChatMessage {
    pub name: Option<String>,
    pub text: Option<String>,
    pub create_time: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleChatUser {
    pub name: String,
    pub display_name: Option<String>,
    #[serde(rename = "type")]
    pub user_type: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_name() {
        let (tx, _rx) = mpsc::channel(1);
        let ch = GoogleChatChannel::new(GoogleChatConfig::default(), tx);
        assert_eq!(ch.name(), "google_chat");
    }

    #[test]
    fn test_parse_event_message() {
        let body = r#"{
            "type": "MESSAGE",
            "eventTime": "2024-01-01T00:00:00Z",
            "space": {
                "name": "spaces/AAAA",
                "displayName": "Test Space",
                "type": "ROOM"
            },
            "message": {
                "name": "spaces/AAAA/messages/12345",
                "text": "Hello from Google Chat!",
                "createTime": "2024-01-01T00:00:00Z"
            },
            "user": {
                "name": "users/123456789",
                "displayName": "Test User",
                "type": "HUMAN"
            }
        }"#;

        let event = GoogleChatChannel::parse_event(body).unwrap();
        assert_eq!(event.event_type, "MESSAGE");
        assert_eq!(event.space.as_ref().unwrap().name, "spaces/AAAA");
        assert_eq!(
            event.message.as_ref().unwrap().text.as_deref(),
            Some("Hello from Google Chat!")
        );
        assert_eq!(event.user.as_ref().unwrap().name, "users/123456789");
    }

    #[test]
    fn test_parse_event_added_to_space() {
        let body = r#"{
            "type": "ADDED_TO_SPACE",
            "space": {
                "name": "spaces/BBBB",
                "type": "DM"
            },
            "user": {
                "name": "users/987654321",
                "displayName": "Another User"
            }
        }"#;

        let event = GoogleChatChannel::parse_event(body).unwrap();
        assert_eq!(event.event_type, "ADDED_TO_SPACE");
        assert!(event.message.is_none());
    }

    #[test]
    fn test_parse_event_empty_message() {
        let body = r#"{
            "type": "MESSAGE",
            "space": {
                "name": "spaces/CCCC"
            },
            "message": {
                "name": "spaces/CCCC/messages/99999"
            },
            "user": {
                "name": "users/111"
            }
        }"#;

        let event = GoogleChatChannel::parse_event(body).unwrap();
        assert_eq!(event.event_type, "MESSAGE");
        assert!(event.message.as_ref().unwrap().text.is_none());
    }
}
