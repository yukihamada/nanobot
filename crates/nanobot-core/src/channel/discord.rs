use async_trait::async_trait;
use futures::stream::StreamExt;
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::config::DiscordConfig;
use crate::types::{InboundMessage, OutboundMessage};

use super::{is_allowed, Channel};

const DISCORD_API_BASE: &str = "https://discord.com/api/v10";

/// Discord channel using Gateway WebSocket.
pub struct DiscordChannel {
    config: DiscordConfig,
    inbound_tx: mpsc::Sender<InboundMessage>,
    running: bool,
    client: reqwest::Client,
}

impl DiscordChannel {
    pub fn new(config: DiscordConfig, inbound_tx: mpsc::Sender<InboundMessage>) -> Self {
        Self {
            config,
            inbound_tx,
            running: false,
            client: reqwest::Client::new(),
        }
    }

    async fn gateway_loop(&self) -> anyhow::Result<()> {
        use tokio_tungstenite::connect_async;
        use tokio_tungstenite::tungstenite::Message as WsMessage;

        let (ws_stream, _) = connect_async(&self.config.gateway_url).await?;
        let (mut write, mut read) = ws_stream.split();

        let mut _seq: Option<i64> = None;

        while let Some(msg) = read.next().await {
            let msg = msg?;
            if let WsMessage::Text(text) = msg {
                let data: serde_json::Value = serde_json::from_str(&text)?;
                let op = data.get("op").and_then(|v| v.as_i64()).unwrap_or(-1);
                let event_type = data.get("t").and_then(|v| v.as_str());
                let payload = data.get("d");

                if let Some(s) = data.get("s").and_then(|v| v.as_i64()) {
                    _seq = Some(s);
                }

                match op {
                    10 => {
                        // HELLO: identify
                        let _interval_ms = payload
                            .and_then(|p| p.get("heartbeat_interval"))
                            .and_then(|v| v.as_u64())
                            .unwrap_or(45000);

                        // TODO: Start heartbeat in background using interval_ms

                        // Send IDENTIFY
                        let identify = json!({
                            "op": 2,
                            "d": {
                                "token": self.config.token,
                                "intents": self.config.intents,
                                "properties": {
                                    "os": "nanobot",
                                    "browser": "nanobot",
                                    "device": "nanobot",
                                },
                            },
                        });
                        use futures::SinkExt;
                        write
                            .send(WsMessage::Text(serde_json::to_string(&identify)?.into()))
                            .await?;
                    }
                    0 if event_type == Some("READY") => {
                        info!("Discord gateway READY");
                    }
                    0 if event_type == Some("MESSAGE_CREATE") => {
                        if let Some(payload) = payload {
                            self.handle_message_create(payload).await;
                        }
                    }
                    7 | 9 => {
                        info!("Discord gateway requested reconnect");
                        break;
                    }
                    _ => {}
                }
            }
        }

        Ok(())
    }

    async fn handle_message_create(&self, payload: &serde_json::Value) {
        let author = match payload.get("author") {
            Some(a) => a,
            None => return,
        };

        // Skip bots
        if author.get("bot").and_then(|v| v.as_bool()).unwrap_or(false) {
            return;
        }

        let sender_id = author
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let channel_id = payload
            .get("channel_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let content = payload
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("[empty message]")
            .to_string();

        if sender_id.is_empty() || channel_id.is_empty() {
            return;
        }

        if !is_allowed(&sender_id, &self.config.allow_from) {
            return;
        }

        let msg = InboundMessage::new("discord", &sender_id, &channel_id, &content);
        if let Err(e) = self.inbound_tx.send(msg).await {
            error!("Failed to send Discord message to bus: {}", e);
        }
    }
}

#[async_trait]
impl Channel for DiscordChannel {
    fn name(&self) -> &str {
        "discord"
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        if self.config.token.is_empty() {
            return Err(anyhow::anyhow!("Discord bot token not configured"));
        }

        self.running = true;
        info!("Starting Discord gateway...");

        while self.running {
            match self.gateway_loop().await {
                Ok(_) => {}
                Err(e) => {
                    warn!("Discord gateway error: {}", e);
                    if self.running {
                        info!("Reconnecting to Discord gateway in 5 seconds...");
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> anyhow::Result<()> {
        self.running = false;
        info!("Discord channel stopped");
        Ok(())
    }

    async fn send(&self, msg: &OutboundMessage) -> anyhow::Result<()> {
        let url = format!("{}/channels/{}/messages", DISCORD_API_BASE, msg.chat_id);
        let mut payload = json!({"content": msg.content});

        if let Some(ref reply_to) = msg.reply_to {
            payload["message_reference"] = json!({"message_id": reply_to});
            payload["allowed_mentions"] = json!({"replied_user": false});
        }

        for _attempt in 0..3 {
            let response = self
                .client
                .post(&url)
                .header("Authorization", format!("Bot {}", self.config.token))
                .json(&payload)
                .send()
                .await?;

            if response.status().as_u16() == 429 {
                let data: serde_json::Value = response.json().await.unwrap_or_default();
                let retry_after = data
                    .get("retry_after")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(1.0);
                warn!("Discord rate limited, retrying in {}s", retry_after);
                tokio::time::sleep(std::time::Duration::from_secs_f64(retry_after)).await;
                continue;
            }

            response.error_for_status()?;
            return Ok(());
        }

        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running
    }
}
