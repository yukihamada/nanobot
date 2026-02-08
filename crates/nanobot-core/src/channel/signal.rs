use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::config::SignalConfig;
use crate::types::{InboundMessage, OutboundMessage};

use super::{is_allowed, Channel};

/// Signal channel using signal-cli REST API (JSON-RPC + SSE).
pub struct SignalChannel {
    config: SignalConfig,
    inbound_tx: mpsc::Sender<InboundMessage>,
    running: bool,
    client: reqwest::Client,
}

impl SignalChannel {
    pub fn new(config: SignalConfig, inbound_tx: mpsc::Sender<InboundMessage>) -> Self {
        Self {
            config,
            inbound_tx,
            running: false,
            client: reqwest::Client::new(),
        }
    }

    /// Poll for messages via the signal-cli REST API receive endpoint.
    async fn receive_loop(&self) -> anyhow::Result<()> {
        let url = format!(
            "{}/v1/receive/{}",
            self.config.endpoint, self.config.phone_number
        );

        loop {
            if !self.running {
                break;
            }

            let resp = self.client.get(&url).send().await?;

            if !resp.status().is_success() {
                warn!("Signal receive error: {}", resp.status());
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            }

            let messages: Vec<serde_json::Value> = resp.json().await.unwrap_or_default();

            for msg in &messages {
                self.handle_message(msg).await;
            }

            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }

        Ok(())
    }

    /// Handle an incoming Signal message.
    async fn handle_message(&self, raw: &serde_json::Value) {
        let envelope = match raw.get("envelope") {
            Some(e) => e,
            None => return,
        };

        let data_message = match envelope.get("dataMessage") {
            Some(dm) => dm,
            None => return,
        };

        let sender = envelope
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let content = data_message
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if sender.is_empty() || content.is_empty() {
            return;
        }

        // Use group ID if present, otherwise sender as chat_id
        let chat_id = envelope
            .get("dataMessage")
            .and_then(|dm| dm.get("groupInfo"))
            .and_then(|gi| gi.get("groupId"))
            .and_then(|v| v.as_str())
            .unwrap_or(&sender)
            .to_string();

        if !is_allowed(&sender, &self.config.allow_from) {
            return;
        }

        debug!("Signal message from {}: {}", sender, content);

        let msg = InboundMessage::new("signal", &sender, &chat_id, &content);
        if let Err(e) = self.inbound_tx.send(msg).await {
            error!("Failed to send Signal message to bus: {}", e);
        }
    }

    /// Send a message via signal-cli REST API.
    async fn send_message(&self, recipient: &str, text: &str) -> anyhow::Result<()> {
        let url = format!("{}/v2/send", self.config.endpoint);

        let body = json!({
            "message": text,
            "number": self.config.phone_number,
            "recipients": [recipient],
        });

        let resp = self.client.post(&url).json(&body).send().await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("Signal send error: {} {}", status, text));
        }

        Ok(())
    }

    /// Parse a signal-cli receive response.
    pub fn parse_messages(body: &str) -> Result<Vec<SignalEnvelope>, serde_json::Error> {
        serde_json::from_str(body)
    }
}

#[async_trait]
impl Channel for SignalChannel {
    fn name(&self) -> &str {
        "signal"
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        if self.config.phone_number.is_empty() {
            return Err(anyhow::anyhow!("Signal phone number not configured"));
        }

        self.running = true;
        info!("Starting Signal channel...");

        while self.running {
            match self.receive_loop().await {
                Ok(_) => {}
                Err(e) => {
                    warn!("Signal receive error: {}", e);
                    if self.running {
                        info!("Reconnecting to Signal in 5 seconds...");
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> anyhow::Result<()> {
        self.running = false;
        info!("Signal channel stopped");
        Ok(())
    }

    async fn send(&self, msg: &OutboundMessage) -> anyhow::Result<()> {
        self.send_message(&msg.chat_id, &msg.content).await
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

// ====== Signal Types ======

#[derive(Debug, Deserialize)]
pub struct SignalEnvelope {
    pub envelope: Option<SignalEnvelopeData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalEnvelopeData {
    pub source: Option<String>,
    pub data_message: Option<SignalDataMessage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalDataMessage {
    pub message: Option<String>,
    pub group_info: Option<SignalGroupInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalGroupInfo {
    pub group_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_name() {
        let (tx, _rx) = mpsc::channel(1);
        let ch = SignalChannel::new(SignalConfig::default(), tx);
        assert_eq!(ch.name(), "signal");
    }

    #[test]
    fn test_parse_messages() {
        let body = r#"[{
            "envelope": {
                "source": "+1234567890",
                "dataMessage": {
                    "message": "Hello from Signal!",
                    "timestamp": 1625000000000
                }
            }
        }]"#;

        let messages = SignalChannel::parse_messages(body).unwrap();
        assert_eq!(messages.len(), 1);
        let env = messages[0].envelope.as_ref().unwrap();
        assert_eq!(env.source.as_deref(), Some("+1234567890"));
        assert_eq!(
            env.data_message.as_ref().unwrap().message.as_deref(),
            Some("Hello from Signal!")
        );
    }

    #[test]
    fn test_parse_messages_with_group() {
        let body = r#"[{
            "envelope": {
                "source": "+1234567890",
                "dataMessage": {
                    "message": "Group message",
                    "groupInfo": {
                        "groupId": "group123"
                    }
                }
            }
        }]"#;

        let messages = SignalChannel::parse_messages(body).unwrap();
        let env = messages[0].envelope.as_ref().unwrap();
        let dm = env.data_message.as_ref().unwrap();
        assert_eq!(
            dm.group_info.as_ref().unwrap().group_id.as_deref(),
            Some("group123")
        );
    }

    #[test]
    fn test_parse_messages_empty() {
        let body = "[]";
        let messages = SignalChannel::parse_messages(body).unwrap();
        assert!(messages.is_empty());
    }
}
