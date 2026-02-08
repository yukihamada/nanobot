use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{error, info};

use crate::types::OutboundMessage;

use super::Channel;

// ====== Facebook Messenger Webhook Types ======

/// Facebook webhook event envelope.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FacebookWebhookEvent {
    pub object: String,
    pub entry: Vec<FacebookEntry>,
}

/// A single entry in the webhook event.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FacebookEntry {
    pub id: String,
    pub time: i64,
    pub messaging: Option<Vec<FacebookMessaging>>,
}

/// A messaging event.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FacebookMessaging {
    pub sender: FacebookUser,
    pub recipient: FacebookUser,
    pub timestamp: i64,
    pub message: Option<FacebookMessage>,
}

/// Facebook user reference.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FacebookUser {
    pub id: String,
}

/// Facebook message.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FacebookMessage {
    pub mid: Option<String>,
    pub text: Option<String>,
}

// ====== Channel Implementation ======

pub struct FacebookChannel {
    running: bool,
    client: reqwest::Client,
}

impl FacebookChannel {
    pub fn new() -> Self {
        Self {
            running: false,
            client: reqwest::Client::new(),
        }
    }

    /// Parse a Facebook webhook event from JSON body.
    pub fn parse_webhook_event(body: &str) -> Result<FacebookWebhookEvent, serde_json::Error> {
        serde_json::from_str(body)
    }

    /// Send a text message via the Facebook Send API.
    pub async fn send_message_static(
        client: &reqwest::Client,
        page_access_token: &str,
        recipient_id: &str,
        text: &str,
    ) -> anyhow::Result<()> {
        let url = format!(
            "https://graph.facebook.com/v21.0/me/messages?access_token={}",
            page_access_token
        );

        let response = client
            .post(&url)
            .json(&json!({
                "recipient": { "id": recipient_id },
                "message": { "text": text },
                "messaging_type": "RESPONSE",
            }))
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!("Facebook Send API error {}: {}", status, body);
            anyhow::bail!("Facebook Send API returned {}", status);
        }

        Ok(())
    }
}

#[async_trait]
impl Channel for FacebookChannel {
    fn name(&self) -> &str {
        "facebook"
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        info!("Facebook Messenger channel started (webhook mode)");
        Ok(())
    }

    async fn stop(&mut self) -> anyhow::Result<()> {
        self.running = false;
        Ok(())
    }

    async fn send(&self, msg: &OutboundMessage) -> anyhow::Result<()> {
        let token = std::env::var("FACEBOOK_PAGE_ACCESS_TOKEN").unwrap_or_default();
        if token.is_empty() {
            anyhow::bail!("FACEBOOK_PAGE_ACCESS_TOKEN not set");
        }
        Self::send_message_static(&self.client, &token, &msg.chat_id, &msg.content).await
    }

    fn is_running(&self) -> bool {
        self.running
    }
}
