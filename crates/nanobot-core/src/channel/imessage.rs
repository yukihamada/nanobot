use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::config::IMessageConfig;
use crate::types::{InboundMessage, OutboundMessage};

use super::{is_allowed, Channel};

/// iMessage channel using BlueBubbles HTTP REST API.
pub struct IMessageChannel {
    config: IMessageConfig,
    inbound_tx: mpsc::Sender<InboundMessage>,
    running: bool,
    client: reqwest::Client,
}

impl IMessageChannel {
    pub fn new(config: IMessageConfig, inbound_tx: mpsc::Sender<InboundMessage>) -> Self {
        Self {
            config,
            inbound_tx,
            running: false,
            client: reqwest::Client::new(),
        }
    }

    /// Poll for new messages from the BlueBubbles bridge.
    async fn poll_loop(&self) -> anyhow::Result<()> {
        let mut last_timestamp: i64 = chrono::Utc::now().timestamp_millis();

        loop {
            if !self.running {
                break;
            }

            let url = format!(
                "{}/api/v1/message?after={}&sort=asc&limit=100",
                self.config.bridge_url, last_timestamp
            );

            match self.client.get(&url).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        let body: serde_json::Value = resp.json().await.unwrap_or_default();
                        let messages = body
                            .get("data")
                            .and_then(|v| v.as_array())
                            .cloned()
                            .unwrap_or_default();

                        for msg in &messages {
                            self.handle_message(msg).await;
                            if let Some(ts) = msg.get("dateCreated").and_then(|v| v.as_i64()) {
                                if ts > last_timestamp {
                                    last_timestamp = ts;
                                }
                            }
                        }
                    } else {
                        warn!("iMessage bridge error: {}", resp.status());
                    }
                }
                Err(e) => {
                    warn!("iMessage bridge connection error: {}", e);
                }
            }

            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }

        Ok(())
    }

    /// Handle an incoming iMessage.
    async fn handle_message(&self, raw: &serde_json::Value) {
        let is_from_me = raw
            .get("isFromMe")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        if is_from_me {
            return;
        }

        let sender = raw
            .get("handle")
            .and_then(|h| h.get("address"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let content = raw
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if sender.is_empty() || content.is_empty() {
            return;
        }

        let chat_id = raw
            .get("chats")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|c| c.get("guid"))
            .and_then(|v| v.as_str())
            .unwrap_or(&sender)
            .to_string();

        if !is_allowed(&sender, &self.config.allow_from) {
            return;
        }

        debug!("iMessage from {}: {}", sender, content);

        let msg = InboundMessage::new("imessage", &sender, &chat_id, &content);
        if let Err(e) = self.inbound_tx.send(msg).await {
            error!("Failed to send iMessage to bus: {}", e);
        }
    }

    /// Send a text message via BlueBubbles.
    async fn send_text(&self, chat_guid: &str, text: &str) -> anyhow::Result<()> {
        let url = format!("{}/api/v1/message/text", self.config.bridge_url);

        let body = json!({
            "chatGuid": chat_guid,
            "message": text,
        });

        let resp = self.client.post(&url).json(&body).send().await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "iMessage send error: {status} {text}"
            ));
        }

        Ok(())
    }

    /// Parse a BlueBubbles message response.
    pub fn parse_messages(body: &str) -> Result<BlueBubblesResponse, serde_json::Error> {
        serde_json::from_str(body)
    }
}

#[async_trait]
impl Channel for IMessageChannel {
    fn name(&self) -> &str {
        "imessage"
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        if self.config.bridge_url.is_empty() {
            return Err(anyhow::anyhow!("iMessage bridge URL not configured"));
        }

        self.running = true;
        info!("Starting iMessage channel (BlueBubbles polling)...");

        while self.running {
            match self.poll_loop().await {
                Ok(_) => {}
                Err(e) => {
                    warn!("iMessage poll error: {}", e);
                    if self.running {
                        info!("Retrying iMessage poll in 5 seconds...");
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> anyhow::Result<()> {
        self.running = false;
        info!("iMessage channel stopped");
        Ok(())
    }

    async fn send(&self, msg: &OutboundMessage) -> anyhow::Result<()> {
        self.send_text(&msg.chat_id, &msg.content).await
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

// ====== BlueBubbles Types ======

#[derive(Debug, Deserialize)]
pub struct BlueBubblesResponse {
    pub status: Option<i32>,
    pub data: Option<Vec<BlueBubblesMessage>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlueBubblesMessage {
    pub guid: Option<String>,
    pub text: Option<String>,
    pub is_from_me: Option<bool>,
    pub date_created: Option<i64>,
    pub handle: Option<BlueBubblesHandle>,
}

#[derive(Debug, Deserialize)]
pub struct BlueBubblesHandle {
    pub address: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_name() {
        let (tx, _rx) = mpsc::channel(1);
        let ch = IMessageChannel::new(IMessageConfig::default(), tx);
        assert_eq!(ch.name(), "imessage");
    }

    #[test]
    fn test_parse_messages() {
        let body = r#"{
            "status": 200,
            "data": [{
                "guid": "msg-guid-001",
                "text": "Hello from iMessage!",
                "isFromMe": false,
                "dateCreated": 1625000000000,
                "handle": {
                    "address": "+1234567890"
                }
            }]
        }"#;

        let resp = IMessageChannel::parse_messages(body).unwrap();
        assert_eq!(resp.status, Some(200));
        let msgs = resp.data.unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text.as_deref(), Some("Hello from iMessage!"));
        assert_eq!(msgs[0].is_from_me, Some(false));
        assert_eq!(
            msgs[0].handle.as_ref().unwrap().address.as_deref(),
            Some("+1234567890")
        );
    }

    #[test]
    fn test_parse_messages_empty() {
        let body = r#"{"status": 200, "data": []}"#;
        let resp = IMessageChannel::parse_messages(body).unwrap();
        assert!(resp.data.unwrap().is_empty());
    }
}
