use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::config::TeamsConfig;
use crate::types::{InboundMessage, OutboundMessage};

use super::{is_allowed, Channel};

const TEAMS_LOGIN_URL: &str =
    "https://login.microsoftonline.com/botframework.com/oauth2/v2.0/token";

/// MS Teams channel using Bot Framework v4 webhooks.
pub struct TeamsChannel {
    config: TeamsConfig,
    inbound_tx: mpsc::Sender<InboundMessage>,
    running: bool,
    client: reqwest::Client,
    access_token: Option<String>,
}

impl TeamsChannel {
    pub fn new(config: TeamsConfig, inbound_tx: mpsc::Sender<InboundMessage>) -> Self {
        Self {
            config,
            inbound_tx,
            running: false,
            client: reqwest::Client::new(),
            access_token: None,
        }
    }

    /// Get a Bot Framework access token using client credentials.
    async fn get_access_token(&mut self) -> anyhow::Result<String> {
        let resp: serde_json::Value = self
            .client
            .post(TEAMS_LOGIN_URL)
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", &self.config.app_id),
                ("client_secret", &self.config.app_password),
                (
                    "scope",
                    "https://api.botframework.com/.default",
                ),
            ])
            .send()
            .await?
            .json()
            .await?;

        let token = resp
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("No access_token in Teams auth response"))?
            .to_string();

        self.access_token = Some(token.clone());
        Ok(token)
    }

    /// Process an incoming Bot Framework activity (webhook payload).
    pub async fn process_activity(&self, activity: &TeamsActivity) {
        if activity.activity_type != "message" {
            debug!("Ignoring Teams activity type: {}", activity.activity_type);
            return;
        }

        let sender_id = activity
            .from
            .as_ref()
            .map(|f| f.id.as_str())
            .unwrap_or("")
            .to_string();

        let conversation_id = activity
            .conversation
            .as_ref()
            .map(|c| c.id.as_str())
            .unwrap_or("")
            .to_string();

        let content = activity.text.as_deref().unwrap_or("").to_string();

        if sender_id.is_empty() || conversation_id.is_empty() || content.is_empty() {
            return;
        }

        if !is_allowed(&sender_id, &self.config.allow_from) {
            warn!("Teams message from unauthorized user: {}", sender_id);
            return;
        }

        info!("Teams message from {}: {}", sender_id, content);

        let msg = InboundMessage::new("teams", &sender_id, &conversation_id, &content);
        if let Err(e) = self.inbound_tx.send(msg).await {
            error!("Failed to send Teams message to bus: {}", e);
        }
    }

    /// Send a reply to a conversation via Bot Framework REST API.
    async fn reply_to_conversation(
        &self,
        service_url: &str,
        conversation_id: &str,
        text: &str,
    ) -> anyhow::Result<()> {
        let token = self
            .access_token
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Teams: no access token"))?;

        let url = format!(
            "{}v3/conversations/{}/activities",
            service_url, conversation_id
        );

        let body = json!({
            "type": "message",
            "text": text,
        });

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("Teams send error: {} {}", status, text));
        }

        Ok(())
    }

    /// Parse a Bot Framework activity from webhook body.
    pub fn parse_activity(body: &str) -> Result<TeamsActivity, serde_json::Error> {
        serde_json::from_str(body)
    }
}

#[async_trait]
impl Channel for TeamsChannel {
    fn name(&self) -> &str {
        "teams"
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        if self.config.app_id.is_empty() || self.config.app_password.is_empty() {
            return Err(anyhow::anyhow!(
                "Teams app_id and app_password must be configured"
            ));
        }

        self.running = true;
        info!("Starting MS Teams channel (webhook mode)...");

        // Pre-fetch access token
        match self.get_access_token().await {
            Ok(_) => info!("Teams access token acquired"),
            Err(e) => warn!("Failed to get Teams access token: {}", e),
        }

        Ok(())
    }

    async fn stop(&mut self) -> anyhow::Result<()> {
        self.running = false;
        info!("Teams channel stopped");
        Ok(())
    }

    async fn send(&self, msg: &OutboundMessage) -> anyhow::Result<()> {
        // The service_url is stored in metadata by the webhook handler
        let service_url = msg
            .metadata
            .get("service_url")
            .and_then(|v| v.as_str())
            .unwrap_or("https://smba.trafficmanager.net/teams/");

        self.reply_to_conversation(service_url, &msg.chat_id, &msg.content)
            .await
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

// ====== Teams Bot Framework Types ======

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamsActivity {
    #[serde(rename = "type")]
    pub activity_type: String,
    pub id: Option<String>,
    pub text: Option<String>,
    pub from: Option<TeamsAccount>,
    pub conversation: Option<TeamsConversation>,
    pub service_url: Option<String>,
    pub channel_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TeamsAccount {
    pub id: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TeamsConversation {
    pub id: String,
    #[serde(rename = "conversationType")]
    pub conversation_type: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_name() {
        let (tx, _rx) = mpsc::channel(1);
        let ch = TeamsChannel::new(TeamsConfig::default(), tx);
        assert_eq!(ch.name(), "teams");
    }

    #[test]
    fn test_parse_activity_message() {
        let body = r#"{
            "type": "message",
            "id": "activity-id-001",
            "text": "Hello from Teams!",
            "from": {
                "id": "user-id-123",
                "name": "Test User"
            },
            "conversation": {
                "id": "conv-id-456",
                "conversationType": "personal"
            },
            "serviceUrl": "https://smba.trafficmanager.net/teams/",
            "channelId": "msteams"
        }"#;

        let activity = TeamsChannel::parse_activity(body).unwrap();
        assert_eq!(activity.activity_type, "message");
        assert_eq!(activity.text.as_deref(), Some("Hello from Teams!"));
        assert_eq!(activity.from.as_ref().unwrap().id, "user-id-123");
        assert_eq!(
            activity.from.as_ref().unwrap().name.as_deref(),
            Some("Test User")
        );
        assert_eq!(
            activity.conversation.as_ref().unwrap().id,
            "conv-id-456"
        );
    }

    #[test]
    fn test_parse_activity_non_message() {
        let body = r#"{
            "type": "conversationUpdate",
            "from": {
                "id": "bot-id"
            },
            "conversation": {
                "id": "conv-id"
            }
        }"#;

        let activity = TeamsChannel::parse_activity(body).unwrap();
        assert_eq!(activity.activity_type, "conversationUpdate");
        assert!(activity.text.is_none());
    }
}
