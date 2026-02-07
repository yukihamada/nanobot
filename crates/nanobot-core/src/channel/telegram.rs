use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::config::TelegramConfig;
use crate::types::{InboundMessage, OutboundMessage};

use super::{is_allowed, Channel};

// ====== Webhook Types ======

/// Telegram Update object (subset for webhook use).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TelegramUpdate {
    pub update_id: i64,
    pub message: Option<TelegramMessage>,
}

/// Telegram Message object.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TelegramMessage {
    pub message_id: i64,
    pub from: Option<TelegramUser>,
    pub chat: TelegramChat,
    pub text: Option<String>,
    pub caption: Option<String>,
    pub date: i64,
}

/// Telegram Chat object.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TelegramChat {
    pub id: i64,
    #[serde(rename = "type")]
    pub chat_type: String,
}

/// Telegram User object.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TelegramUser {
    pub id: i64,
    pub is_bot: bool,
    pub first_name: String,
    pub username: Option<String>,
}

// ====== Channel Implementation ======

/// Telegram channel using Bot API polling.
pub struct TelegramChannel {
    config: TelegramConfig,
    inbound_tx: mpsc::Sender<InboundMessage>,
    running: bool,
    client: reqwest::Client,
}

impl TelegramChannel {
    pub fn new(config: TelegramConfig, inbound_tx: mpsc::Sender<InboundMessage>) -> Self {
        Self {
            config,
            inbound_tx,
            running: false,
            client: reqwest::Client::new(),
        }
    }

    fn api_url(&self, method: &str) -> String {
        format!(
            "https://api.telegram.org/bot{}/{}",
            self.config.token, method
        )
    }

    /// Build a Telegram API URL from a token and method name.
    fn api_url_with_token(token: &str, method: &str) -> String {
        format!("https://api.telegram.org/bot{}/{}", token, method)
    }

    async fn get_updates(&self, offset: i64) -> anyhow::Result<Vec<serde_json::Value>> {
        let response: serde_json::Value = self
            .client
            .post(&self.api_url("getUpdates"))
            .json(&json!({
                "offset": offset,
                "timeout": 30,
                "allowed_updates": ["message"],
            }))
            .send()
            .await?
            .json()
            .await?;

        Ok(response
            .get("result")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default())
    }

    async fn send_message(&self, chat_id: &str, text: &str) -> anyhow::Result<()> {
        Self::send_message_static(&self.client, &self.config.token, chat_id, text).await
    }

    /// Parse a Telegram webhook update from JSON body.
    pub fn parse_webhook_update(body: &str) -> Result<TelegramUpdate, serde_json::Error> {
        serde_json::from_str(body)
    }

    /// Send a message using the Telegram Bot API (static version for webhook use).
    pub async fn send_message_static(
        client: &reqwest::Client,
        token: &str,
        chat_id: &str,
        text: &str,
    ) -> anyhow::Result<()> {
        let url = Self::api_url_with_token(token, "sendMessage");

        let response = client
            .post(&url)
            .json(&json!({
                "chat_id": chat_id,
                "text": text,
                "parse_mode": "HTML",
            }))
            .send()
            .await?;

        if !response.status().is_success() {
            // Fallback to plain text
            warn!("HTML parse failed, falling back to plain text");
            client
                .post(&url)
                .json(&json!({
                    "chat_id": chat_id,
                    "text": text,
                }))
                .send()
                .await?;
        }

        Ok(())
    }

    /// Register a webhook URL with the Telegram Bot API.
    pub async fn set_webhook(token: &str, webhook_url: &str) -> anyhow::Result<()> {
        let client = reqwest::Client::new();
        let url = Self::api_url_with_token(token, "setWebhook");

        let resp = client
            .post(&url)
            .json(&json!({
                "url": webhook_url,
                "allowed_updates": ["message"],
            }))
            .send()
            .await?;

        let status = resp.status();
        let body: serde_json::Value = resp.json().await?;

        if !status.is_success() || body.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            return Err(anyhow::anyhow!(
                "Failed to set webhook: {}",
                body.get("description").and_then(|v| v.as_str()).unwrap_or("unknown error")
            ));
        }

        info!("Telegram webhook set to {}", webhook_url);
        Ok(())
    }

    /// Remove the webhook URL from the Telegram Bot API.
    pub async fn delete_webhook(token: &str) -> anyhow::Result<()> {
        let client = reqwest::Client::new();
        let url = Self::api_url_with_token(token, "deleteWebhook");

        let resp = client.post(&url).send().await?;
        let status = resp.status();
        let body: serde_json::Value = resp.json().await?;

        if !status.is_success() || body.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            return Err(anyhow::anyhow!(
                "Failed to delete webhook: {}",
                body.get("description").and_then(|v| v.as_str()).unwrap_or("unknown error")
            ));
        }

        info!("Telegram webhook deleted");
        Ok(())
    }

    async fn handle_update(&self, update: &serde_json::Value) -> anyhow::Result<()> {
        let message = match update.get("message") {
            Some(m) => m,
            None => return Ok(()),
        };

        let from = match message.get("from") {
            Some(f) => f,
            None => return Ok(()),
        };

        let user_id = from.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
        let username = from.get("username").and_then(|v| v.as_str()).unwrap_or("");
        let chat_id = message
            .get("chat")
            .and_then(|c| c.get("id"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        let sender_id = if username.is_empty() {
            user_id.to_string()
        } else {
            format!("{}|{}", user_id, username)
        };

        if !is_allowed(&sender_id, &self.config.allow_from) {
            warn!("Access denied for sender {} on telegram", sender_id);
            return Ok(());
        }

        let text = message
            .get("text")
            .or_else(|| message.get("caption"))
            .and_then(|v| v.as_str())
            .unwrap_or("[empty message]");

        debug!("Telegram message from {}: {}...", sender_id, &text[..text.len().min(50)]);

        let msg = InboundMessage::new("telegram", &sender_id, &chat_id.to_string(), text);
        self.inbound_tx.send(msg).await?;

        Ok(())
    }
}

#[async_trait]
impl Channel for TelegramChannel {
    fn name(&self) -> &str {
        "telegram"
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        if self.config.token.is_empty() {
            return Err(anyhow::anyhow!("Telegram bot token not configured"));
        }

        self.running = true;
        info!("Starting Telegram bot (polling mode)...");

        let mut offset: i64 = 0;
        while self.running {
            match self.get_updates(offset).await {
                Ok(updates) => {
                    for update in &updates {
                        if let Some(id) = update.get("update_id").and_then(|v| v.as_i64()) {
                            offset = id + 1;
                        }
                        if let Err(e) = self.handle_update(update).await {
                            error!("Error handling Telegram update: {}", e);
                        }
                    }
                }
                Err(e) => {
                    warn!("Telegram polling error: {}", e);
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> anyhow::Result<()> {
        self.running = false;
        info!("Telegram bot stopped");
        Ok(())
    }

    async fn send(&self, msg: &OutboundMessage) -> anyhow::Result<()> {
        self.send_message(&msg.chat_id, &msg.content).await
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_webhook_update_text_message() {
        let body = r#"{
            "update_id": 123456,
            "message": {
                "message_id": 1,
                "from": {
                    "id": 99999,
                    "is_bot": false,
                    "first_name": "Test",
                    "username": "testuser"
                },
                "chat": {
                    "id": 99999,
                    "type": "private"
                },
                "text": "Hello bot!",
                "date": 1700000000
            }
        }"#;

        let update = TelegramChannel::parse_webhook_update(body).unwrap();
        assert_eq!(update.update_id, 123456);
        let msg = update.message.unwrap();
        assert_eq!(msg.text.as_deref(), Some("Hello bot!"));
        assert_eq!(msg.chat.id, 99999);
        assert_eq!(msg.from.as_ref().unwrap().username.as_deref(), Some("testuser"));
    }

    #[test]
    fn test_parse_webhook_update_no_message() {
        let body = r#"{
            "update_id": 123457
        }"#;

        let update = TelegramChannel::parse_webhook_update(body).unwrap();
        assert_eq!(update.update_id, 123457);
        assert!(update.message.is_none());
    }

    #[test]
    fn test_parse_webhook_update_with_caption() {
        let body = r#"{
            "update_id": 123458,
            "message": {
                "message_id": 2,
                "from": {
                    "id": 11111,
                    "is_bot": false,
                    "first_name": "User"
                },
                "chat": {
                    "id": 11111,
                    "type": "private"
                },
                "caption": "Photo caption",
                "date": 1700000001
            }
        }"#;

        let update = TelegramChannel::parse_webhook_update(body).unwrap();
        let msg = update.message.unwrap();
        assert!(msg.text.is_none());
        assert_eq!(msg.caption.as_deref(), Some("Photo caption"));
    }

    #[test]
    fn test_parse_webhook_update_group_chat() {
        let body = r#"{
            "update_id": 123459,
            "message": {
                "message_id": 3,
                "from": {
                    "id": 22222,
                    "is_bot": false,
                    "first_name": "Alice",
                    "username": "alice"
                },
                "chat": {
                    "id": -100123456,
                    "type": "supergroup"
                },
                "text": "Hello group!",
                "date": 1700000002
            }
        }"#;

        let update = TelegramChannel::parse_webhook_update(body).unwrap();
        let msg = update.message.unwrap();
        assert_eq!(msg.chat.id, -100123456);
        assert_eq!(msg.chat.chat_type, "supergroup");
    }

    #[test]
    fn test_api_url_with_token() {
        let url = TelegramChannel::api_url_with_token("TOKEN123", "sendMessage");
        assert_eq!(url, "https://api.telegram.org/botTOKEN123/sendMessage");
    }
}
