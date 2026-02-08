use async_trait::async_trait;
use futures::stream::StreamExt;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::config::SlackConfig;
use crate::types::{InboundMessage, OutboundMessage};

use super::{is_allowed, Channel};

const SLACK_API_BASE: &str = "https://slack.com/api";

/// Slack channel using Socket Mode (WebSocket) for receiving and Web API for sending.
pub struct SlackChannel {
    config: SlackConfig,
    inbound_tx: mpsc::Sender<InboundMessage>,
    running: bool,
    client: reqwest::Client,
}

impl SlackChannel {
    pub fn new(config: SlackConfig, inbound_tx: mpsc::Sender<InboundMessage>) -> Self {
        Self {
            config,
            inbound_tx,
            running: false,
            client: reqwest::Client::new(),
        }
    }

    /// Open a WebSocket connection URL via apps.connections.open.
    async fn get_ws_url(&self) -> anyhow::Result<String> {
        let resp: serde_json::Value = self
            .client
            .post(format!("{}/apps.connections.open", SLACK_API_BASE))
            .header("Authorization", format!("Bearer {}", self.config.app_token))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send()
            .await?
            .json()
            .await?;

        if resp.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let err = resp
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            return Err(anyhow::anyhow!("Slack connections.open failed: {}", err));
        }

        resp.get("url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("No URL in connections.open response"))
    }

    /// Run the Socket Mode WebSocket loop.
    async fn socket_loop(&self) -> anyhow::Result<()> {
        use futures::SinkExt;
        use tokio_tungstenite::connect_async;
        use tokio_tungstenite::tungstenite::Message as WsMessage;

        let ws_url = self.get_ws_url().await?;
        let (ws_stream, _) = connect_async(&ws_url).await?;
        let (mut write, mut read) = ws_stream.split();

        info!("Slack Socket Mode connected");

        while let Some(msg) = read.next().await {
            let msg = msg?;
            if let WsMessage::Text(text) = msg {
                let data: serde_json::Value = serde_json::from_str(&text)?;
                let msg_type = data.get("type").and_then(|v| v.as_str()).unwrap_or("");

                // Acknowledge envelope
                if let Some(envelope_id) = data.get("envelope_id").and_then(|v| v.as_str()) {
                    let ack = json!({"envelope_id": envelope_id});
                    write
                        .send(WsMessage::Text(serde_json::to_string(&ack)?.into()))
                        .await?;
                }

                match msg_type {
                    "hello" => {
                        info!("Slack Socket Mode hello received");
                    }
                    "events_api" => {
                        if let Some(payload) = data.get("payload") {
                            self.handle_event(payload).await;
                        }
                    }
                    "disconnect" => {
                        info!("Slack Socket Mode disconnect requested");
                        break;
                    }
                    _ => {
                        debug!("Slack Socket Mode unknown type: {}", msg_type);
                    }
                }
            }
        }

        Ok(())
    }

    /// Handle a Slack Events API payload.
    async fn handle_event(&self, payload: &serde_json::Value) {
        let event = match payload.get("event") {
            Some(e) => e,
            None => return,
        };

        let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if event_type != "message" {
            return;
        }

        // Skip bot messages and subtypes (edits, joins, etc.)
        if event.get("subtype").is_some() || event.get("bot_id").is_some() {
            return;
        }

        let sender_id = event
            .get("user")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let channel_id = event
            .get("channel")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let content = event
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if sender_id.is_empty() || channel_id.is_empty() {
            return;
        }

        if !is_allowed(&sender_id, &self.config.allow_from) {
            return;
        }

        let msg = InboundMessage::new("slack", &sender_id, &channel_id, &content);
        if let Err(e) = self.inbound_tx.send(msg).await {
            error!("Failed to send Slack message to bus: {}", e);
        }
    }

    /// Send a message via Slack Web API chat.postMessage.
    async fn post_message(&self, channel: &str, text: &str) -> anyhow::Result<()> {
        let url = format!("{}/chat.postMessage", SLACK_API_BASE);

        for _attempt in 0..3 {
            let resp = self
                .client
                .post(&url)
                .header("Authorization", format!("Bearer {}", self.config.bot_token))
                .json(&json!({
                    "channel": channel,
                    "text": text,
                }))
                .send()
                .await?;

            if resp.status().as_u16() == 429 {
                let retry_after = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(1);
                warn!("Slack rate limited, retrying in {}s", retry_after);
                tokio::time::sleep(std::time::Duration::from_secs(retry_after)).await;
                continue;
            }

            let body: serde_json::Value = resp.json().await?;
            if body.get("ok").and_then(|v| v.as_bool()) != Some(true) {
                let err = body
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                return Err(anyhow::anyhow!("Slack chat.postMessage error: {}", err));
            }
            return Ok(());
        }

        Ok(())
    }

    /// Parse a Slack Events API webhook payload.
    pub fn parse_event(body: &str) -> Result<SlackEventPayload, serde_json::Error> {
        serde_json::from_str(body)
    }
}

#[async_trait]
impl Channel for SlackChannel {
    fn name(&self) -> &str {
        "slack"
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        if self.config.app_token.is_empty() || self.config.bot_token.is_empty() {
            return Err(anyhow::anyhow!(
                "Slack app_token and bot_token must be configured"
            ));
        }

        self.running = true;
        info!("Starting Slack channel (Socket Mode)...");

        while self.running {
            match self.socket_loop().await {
                Ok(_) => {}
                Err(e) => {
                    warn!("Slack Socket Mode error: {}", e);
                    if self.running {
                        info!("Reconnecting to Slack in 5 seconds...");
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> anyhow::Result<()> {
        self.running = false;
        info!("Slack channel stopped");
        Ok(())
    }

    async fn send(&self, msg: &OutboundMessage) -> anyhow::Result<()> {
        self.post_message(&msg.chat_id, &msg.content).await
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

// ====== Slack Event Types ======

#[derive(Debug, Deserialize)]
pub struct SlackEventPayload {
    #[serde(rename = "type")]
    pub payload_type: String,
    pub event: Option<SlackEvent>,
    pub challenge: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SlackEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub user: Option<String>,
    pub channel: Option<String>,
    pub text: Option<String>,
    pub ts: Option<String>,
    pub subtype: Option<String>,
    pub bot_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_name() {
        let (tx, _rx) = mpsc::channel(1);
        let ch = SlackChannel::new(SlackConfig::default(), tx);
        assert_eq!(ch.name(), "slack");
    }

    #[test]
    fn test_parse_event_message() {
        let body = r#"{
            "type": "event_callback",
            "event": {
                "type": "message",
                "user": "U12345",
                "channel": "C12345",
                "text": "Hello from Slack!",
                "ts": "1625000000.000100"
            }
        }"#;

        let payload = SlackChannel::parse_event(body).unwrap();
        assert_eq!(payload.payload_type, "event_callback");
        let event = payload.event.unwrap();
        assert_eq!(event.event_type, "message");
        assert_eq!(event.user.as_deref(), Some("U12345"));
        assert_eq!(event.channel.as_deref(), Some("C12345"));
        assert_eq!(event.text.as_deref(), Some("Hello from Slack!"));
    }

    #[test]
    fn test_parse_event_url_verification() {
        let body = r#"{
            "type": "url_verification",
            "challenge": "challenge_token_123"
        }"#;

        let payload = SlackChannel::parse_event(body).unwrap();
        assert_eq!(payload.payload_type, "url_verification");
        assert_eq!(
            payload.challenge.as_deref(),
            Some("challenge_token_123")
        );
    }

    #[test]
    fn test_parse_event_bot_message() {
        let body = r#"{
            "type": "event_callback",
            "event": {
                "type": "message",
                "bot_id": "B12345",
                "channel": "C12345",
                "text": "Bot message",
                "ts": "1625000001.000200"
            }
        }"#;

        let payload = SlackChannel::parse_event(body).unwrap();
        let event = payload.event.unwrap();
        assert!(event.bot_id.is_some());
    }
}
