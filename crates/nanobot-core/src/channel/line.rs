use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::channel::{is_allowed, Channel};
use crate::config::LineConfig;
use crate::types::{InboundMessage, OutboundMessage};
use crate::util::http::client;

const LINE_REPLY_API: &str = "https://api.line.me/v2/bot/message/reply";
const LINE_PUSH_API: &str = "https://api.line.me/v2/bot/message/push";

/// LINE Messaging API channel.
pub struct LineChannel {
    config: LineConfig,
    inbound_tx: mpsc::Sender<InboundMessage>,
    running: bool,
}

impl LineChannel {
    pub fn new(config: LineConfig, inbound_tx: mpsc::Sender<InboundMessage>) -> Self {
        Self {
            config,
            inbound_tx,
            running: false,
        }
    }

    /// Verify webhook signature using HMAC-SHA256.
    pub fn verify_signature(channel_secret: &str, body: &[u8], signature: &str) -> bool {
        #[cfg(feature = "http-api")]
        {
            use hmac::{Hmac, Mac};
            use sha2::Sha256;

            type HmacSha256 = Hmac<Sha256>;

            let Ok(mut mac) = HmacSha256::new_from_slice(channel_secret.as_bytes()) else {
                return false;
            };
            mac.update(body);

            let expected = base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                mac.finalize().into_bytes(),
            );
            expected == signature
        }
        #[cfg(not(feature = "http-api"))]
        {
            let _ = (channel_secret, body, signature);
            false
        }
    }

    /// Parse LINE webhook events from the request body.
    pub fn parse_webhook_events(body: &str) -> Result<Vec<LineEvent>, serde_json::Error> {
        let webhook: LineWebhook = serde_json::from_str(body)?;
        Ok(webhook.events)
    }

    /// Reply to a LINE message using the reply token.
    /// Must be called within 1 minute of receiving the webhook.
    pub async fn reply(access_token: &str, reply_token: &str, text: &str) -> anyhow::Result<()> {
        let client = client();
        let body = serde_json::json!({
            "replyToken": reply_token,
            "messages": [{
                "type": "text",
                "text": text
            }]
        });

        let resp = client
            .post(LINE_REPLY_API)
            .header("Authorization", format!("Bearer {access_token}"))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            error!("LINE reply API error: {} {}", status, text);
            return Err(anyhow::anyhow!("LINE reply API error: {status}"));
        }

        debug!("LINE reply sent successfully");
        Ok(())
    }

    /// Push a message to a LINE user/group (no reply token needed).
    pub async fn push_message(access_token: &str, to: &str, text: &str) -> anyhow::Result<()> {
        let client = client();
        let body = serde_json::json!({
            "to": to,
            "messages": [{
                "type": "text",
                "text": text
            }]
        });

        let resp = client
            .post(LINE_PUSH_API)
            .header("Authorization", format!("Bearer {access_token}"))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            error!("LINE push API error: {} {}", status, text);
            return Err(anyhow::anyhow!("LINE push API error: {status}"));
        }

        debug!("LINE push message sent to {}", to);
        Ok(())
    }

    /// Process a LINE webhook event and forward to the agent via inbound_tx.
    pub async fn process_event(&self, event: &LineEvent) {
        match &event.event_type[..] {
            "message" => {
                if let Some(ref message) = event.message {
                    if message.msg_type != "text" {
                        debug!("Ignoring non-text LINE message: {}", message.msg_type);
                        return;
                    }

                    let sender_id = event
                        .source
                        .as_ref()
                        .map(|s| s.user_id.as_deref().unwrap_or("unknown"))
                        .unwrap_or("unknown");

                    if !is_allowed(sender_id, &self.config.allow_from) {
                        warn!("LINE message from unauthorized user: {}", sender_id);
                        return;
                    }

                    let chat_id = event
                        .source
                        .as_ref()
                        .map(|s| {
                            s.group_id
                                .as_deref()
                                .or(s.room_id.as_deref())
                                .or(s.user_id.as_deref())
                                .unwrap_or("unknown")
                        })
                        .unwrap_or("unknown");

                    let text = message.text.as_deref().unwrap_or("");

                    info!("LINE message from {}: {}", sender_id, text);

                    let msg = InboundMessage::new("line", sender_id, chat_id, text);

                    if let Err(e) = self.inbound_tx.send(msg).await {
                        error!("Failed to forward LINE message: {}", e);
                    }
                }
            }
            "follow" => {
                info!("LINE follow event");
            }
            "unfollow" => {
                info!("LINE unfollow event");
            }
            _ => {
                debug!("Ignoring LINE event type: {}", event.event_type);
            }
        }
    }
}

#[async_trait]
impl Channel for LineChannel {
    fn name(&self) -> &str {
        "line"
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        // LINE uses webhooks, so no persistent connection needed.
        // The HTTP server handles incoming webhooks.
        info!("LINE channel started (webhook mode)");
        self.running = true;
        Ok(())
    }

    async fn stop(&mut self) -> anyhow::Result<()> {
        self.running = false;
        info!("LINE channel stopped");
        Ok(())
    }

    async fn send(&self, msg: &OutboundMessage) -> anyhow::Result<()> {
        Self::push_message(&self.config.channel_access_token, &msg.chat_id, &msg.content).await
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

// ====== LINE Webhook Types ======

#[derive(Debug, Deserialize)]
pub struct LineWebhook {
    #[serde(default)]
    pub events: Vec<LineEvent>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub reply_token: Option<String>,
    pub source: Option<LineSource>,
    pub message: Option<LineMessage>,
    pub timestamp: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineSource {
    #[serde(rename = "type")]
    pub source_type: String,
    pub user_id: Option<String>,
    pub group_id: Option<String>,
    pub room_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineMessage {
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub msg_type: String,
    pub text: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_webhook_text_message() {
        let body = r#"{
            "events": [{
                "type": "message",
                "replyToken": "token123",
                "source": {
                    "type": "user",
                    "userId": "U1234567890"
                },
                "message": {
                    "id": "msg001",
                    "type": "text",
                    "text": "Hello!"
                },
                "timestamp": 1625000000000
            }]
        }"#;

        let events = LineChannel::parse_webhook_events(body).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "message");
        assert_eq!(events[0].reply_token.as_deref(), Some("token123"));
        assert_eq!(
            events[0].source.as_ref().unwrap().user_id.as_deref(),
            Some("U1234567890")
        );
        assert_eq!(
            events[0].message.as_ref().unwrap().text.as_deref(),
            Some("Hello!")
        );
    }

    #[test]
    fn test_parse_webhook_follow_event() {
        let body = r#"{
            "events": [{
                "type": "follow",
                "source": {
                    "type": "user",
                    "userId": "U9999"
                },
                "timestamp": 1625000001000
            }]
        }"#;

        let events = LineChannel::parse_webhook_events(body).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "follow");
    }

    #[test]
    fn test_parse_webhook_empty() {
        let body = r#"{"events": []}"#;
        let events = LineChannel::parse_webhook_events(body).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn test_parse_webhook_group_message() {
        let body = r#"{
            "events": [{
                "type": "message",
                "replyToken": "reply123",
                "source": {
                    "type": "group",
                    "groupId": "Gxyz",
                    "userId": "U1234"
                },
                "message": {
                    "id": "msg002",
                    "type": "text",
                    "text": "Hi from group"
                }
            }]
        }"#;

        let events = LineChannel::parse_webhook_events(body).unwrap();
        assert_eq!(events.len(), 1);
        let source = events[0].source.as_ref().unwrap();
        assert_eq!(source.source_type, "group");
        assert_eq!(source.group_id.as_deref(), Some("Gxyz"));
        assert_eq!(source.user_id.as_deref(), Some("U1234"));
    }
}
