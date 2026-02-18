use async_trait::async_trait;
use futures::stream::StreamExt;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::config::WhatsAppConfig;
use crate::types::{InboundMessage, OutboundMessage};

use super::{is_allowed, Channel};

/// WhatsApp channel that connects to a Node.js bridge.
pub struct WhatsAppChannel {
    config: WhatsAppConfig,
    inbound_tx: mpsc::Sender<InboundMessage>,
    running: bool,
}

impl WhatsAppChannel {
    pub fn new(config: WhatsAppConfig, inbound_tx: mpsc::Sender<InboundMessage>) -> Self {
        Self {
            config,
            inbound_tx,
            running: false,
        }
    }

    async fn handle_bridge_message(&self, raw: &str) -> anyhow::Result<()> {
        let data: serde_json::Value = serde_json::from_str(raw)?;
        let msg_type = data.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match msg_type {
            "message" => {
                let sender = data
                    .get("sender")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let content = data
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let chat_id = if sender.contains('@') {
                    sender.split('@').next().unwrap_or(sender)
                } else {
                    sender
                };

                if !is_allowed(chat_id, &self.config.allow_from) {
                    warn!("Access denied for sender {} on whatsapp", chat_id);
                    return Ok(());
                }

                let msg = InboundMessage::new("whatsapp", chat_id, sender, content);
                self.inbound_tx.send(msg).await?;
            }
            "status" => {
                let status = data.get("status").and_then(|v| v.as_str()).unwrap_or("");
                info!("WhatsApp status: {}", status);
            }
            "qr" => {
                info!("Scan QR code in the bridge terminal to connect WhatsApp");
            }
            "error" => {
                let err = data.get("error").and_then(|v| v.as_str()).unwrap_or("");
                error!("WhatsApp bridge error: {}", err);
            }
            _ => {}
        }

        Ok(())
    }
}

#[async_trait]
impl Channel for WhatsAppChannel {
    fn name(&self) -> &str {
        "whatsapp"
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        use tokio_tungstenite::connect_async;
        use tokio_tungstenite::tungstenite::Message as WsMessage;

        self.running = true;
        info!("Connecting to WhatsApp bridge at {}...", self.config.bridge_url);

        while self.running {
            match connect_async(&self.config.bridge_url).await {
                Ok((ws_stream, _)) => {
                    info!("Connected to WhatsApp bridge");
                    let (_, mut read) = ws_stream.split();

                    while let Some(msg) = read.next().await {
                        match msg {
                            Ok(WsMessage::Text(text)) => {
                                if let Err(e) = self.handle_bridge_message(&text).await {
                                    error!("Error handling bridge message: {}", e);
                                }
                            }
                            Err(e) => {
                                warn!("WhatsApp WebSocket error: {}", e);
                                break;
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) => {
                    warn!("WhatsApp bridge connection error: {}", e);
                }
            }

            if self.running {
                info!("Reconnecting in 5 seconds...");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> anyhow::Result<()> {
        self.running = false;
        info!("WhatsApp channel stopped");
        Ok(())
    }

    async fn send(&self, _msg: &OutboundMessage) -> anyhow::Result<()> {
        // Would need a stored WebSocket write handle for sending
        // For now, log a warning
        warn!("WhatsApp send not implemented in this context - bridge connection needed");
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running
    }
}
