use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::config::FeishuConfig;
use crate::types::{InboundMessage, OutboundMessage};

use super::Channel;

/// Feishu/Lark channel.
///
/// Note: The full Feishu SDK integration requires the `lark-oapi` crate
/// which doesn't have a Rust equivalent. This implementation uses the
/// REST API directly for sending messages. For receiving, it would need
/// a WebSocket connection or webhook setup.
#[allow(dead_code)]
pub struct FeishuChannel {
    config: FeishuConfig,
    inbound_tx: mpsc::Sender<InboundMessage>,
    running: bool,
    client: reqwest::Client,
    access_token: Option<String>,
}

impl FeishuChannel {
    pub fn new(config: FeishuConfig, inbound_tx: mpsc::Sender<InboundMessage>) -> Self {
        Self {
            config,
            inbound_tx,
            running: false,
            client: reqwest::Client::new(),
            access_token: None,
        }
    }

    /// Get tenant access token from Feishu.
    async fn get_access_token(&mut self) -> anyhow::Result<String> {
        if let Some(ref token) = self.access_token {
            return Ok(token.clone());
        }

        let response: serde_json::Value = self
            .client
            .post("https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal")
            .json(&json!({
                "app_id": self.config.app_id,
                "app_secret": self.config.app_secret,
            }))
            .send()
            .await?
            .json()
            .await?;

        let token = response
            .get("tenant_access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Failed to get Feishu access token"))?
            .to_string();

        self.access_token = Some(token.clone());
        Ok(token)
    }
}

#[async_trait]
impl Channel for FeishuChannel {
    fn name(&self) -> &str {
        "feishu"
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        if self.config.app_id.is_empty() || self.config.app_secret.is_empty() {
            return Err(anyhow::anyhow!(
                "Feishu app_id and app_secret not configured"
            ));
        }

        self.running = true;
        info!("Starting Feishu bot...");

        // Get initial access token
        self.get_access_token().await?;
        info!("Feishu bot started (REST API mode)");

        // Keep running - in production this would use WebSocket long connection
        while self.running {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }

        Ok(())
    }

    async fn stop(&mut self) -> anyhow::Result<()> {
        self.running = false;
        info!("Feishu bot stopped");
        Ok(())
    }

    async fn send(&self, msg: &OutboundMessage) -> anyhow::Result<()> {
        let token = self
            .access_token
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No access token"))?;

        let receive_id_type = if msg.chat_id.starts_with("oc_") {
            "chat_id"
        } else {
            "open_id"
        };

        let content = json!({"text": msg.content}).to_string();

        let response = self
            .client
            .post(format!(
                "https://open.feishu.cn/open-apis/im/v1/messages?receive_id_type={receive_id_type}"
            ))
            .header("Authorization", format!("Bearer {token}"))
            .json(&json!({
                "receive_id": msg.chat_id,
                "msg_type": "text",
                "content": content,
            }))
            .send()
            .await?;

        if !response.status().is_success() {
            let text = response.text().await.unwrap_or_default();
            error!("Failed to send Feishu message: {}", text);
        }

        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running
    }
}
