use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::config::MatrixConfig;
use crate::types::{InboundMessage, OutboundMessage};

use super::{is_allowed, Channel};

/// Matrix channel using the Client-Server API (REST long-polling via /sync).
pub struct MatrixChannel {
    config: MatrixConfig,
    inbound_tx: mpsc::Sender<InboundMessage>,
    running: bool,
    client: reqwest::Client,
}

impl MatrixChannel {
    pub fn new(config: MatrixConfig, inbound_tx: mpsc::Sender<InboundMessage>) -> Self {
        Self {
            config,
            inbound_tx,
            running: false,
            client: reqwest::Client::new(),
        }
    }

    /// Run the /sync long-polling loop.
    async fn sync_loop(&self) -> anyhow::Result<()> {
        let mut next_batch: Option<String> = None;

        // Initial sync to get the since token (don't process old messages)
        let initial_url = format!(
            "{}/_matrix/client/v3/sync?timeout=0",
            self.config.homeserver
        );
        let resp: serde_json::Value = self
            .client
            .get(&initial_url)
            .header(
                "Authorization",
                format!("Bearer {}", self.config.access_token),
            )
            .send()
            .await?
            .json()
            .await?;

        if let Some(nb) = resp.get("next_batch").and_then(|v| v.as_str()) {
            next_batch = Some(nb.to_string());
        }

        info!("Matrix initial sync complete");

        loop {
            if !self.running {
                break;
            }

            let url = match &next_batch {
                Some(since) => format!(
                    "{}/_matrix/client/v3/sync?since={}&timeout=30000",
                    self.config.homeserver, since
                ),
                None => format!(
                    "{}/_matrix/client/v3/sync?timeout=30000",
                    self.config.homeserver
                ),
            };

            let resp: serde_json::Value = self
                .client
                .get(&url)
                .header(
                    "Authorization",
                    format!("Bearer {}", self.config.access_token),
                )
                .send()
                .await?
                .json()
                .await?;

            if let Some(nb) = resp.get("next_batch").and_then(|v| v.as_str()) {
                next_batch = Some(nb.to_string());
            }

            // Process joined rooms
            if let Some(rooms) = resp
                .get("rooms")
                .and_then(|r| r.get("join"))
                .and_then(|j| j.as_object())
            {
                for (room_id, room_data) in rooms {
                    if let Some(events) = room_data
                        .get("timeline")
                        .and_then(|t| t.get("events"))
                        .and_then(|e| e.as_array())
                    {
                        for event in events {
                            self.handle_event(room_id, event).await;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Handle a single Matrix room event.
    async fn handle_event(&self, room_id: &str, event: &serde_json::Value) {
        let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if event_type != "m.room.message" {
            return;
        }

        let sender = event
            .get("sender")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Skip our own messages
        if sender == self.config.user_id {
            return;
        }

        let content = event
            .get("content")
            .and_then(|c| c.get("body"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if sender.is_empty() || content.is_empty() {
            return;
        }

        if !is_allowed(&sender, &self.config.allow_from) {
            return;
        }

        debug!("Matrix message from {} in {}: {}", sender, room_id, content);

        let msg = InboundMessage::new("matrix", &sender, room_id, &content);
        if let Err(e) = self.inbound_tx.send(msg).await {
            error!("Failed to send Matrix message to bus: {}", e);
        }
    }

    /// Send a message to a Matrix room.
    async fn send_to_room(&self, room_id: &str, text: &str) -> anyhow::Result<()> {
        let txn_id = uuid::Uuid::new_v4().to_string();
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/send/m.room.message/{}",
            self.config.homeserver, room_id, txn_id
        );

        let body = json!({
            "msgtype": "m.text",
            "body": text,
        });

        let resp = self
            .client
            .put(&url)
            .header(
                "Authorization",
                format!("Bearer {}", self.config.access_token),
            )
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("Matrix send error: {status} {text}"));
        }

        Ok(())
    }

    /// Parse a Matrix /sync response.
    pub fn parse_sync_response(body: &str) -> Result<MatrixSyncResponse, serde_json::Error> {
        serde_json::from_str(body)
    }
}

#[async_trait]
impl Channel for MatrixChannel {
    fn name(&self) -> &str {
        "matrix"
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        if self.config.access_token.is_empty() || self.config.homeserver.is_empty() {
            return Err(anyhow::anyhow!(
                "Matrix homeserver and access_token must be configured"
            ));
        }

        self.running = true;
        info!("Starting Matrix channel (/sync polling)...");

        while self.running {
            match self.sync_loop().await {
                Ok(_) => {}
                Err(e) => {
                    warn!("Matrix sync error: {}", e);
                    if self.running {
                        info!("Reconnecting to Matrix in 5 seconds...");
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> anyhow::Result<()> {
        self.running = false;
        info!("Matrix channel stopped");
        Ok(())
    }

    async fn send(&self, msg: &OutboundMessage) -> anyhow::Result<()> {
        self.send_to_room(&msg.chat_id, &msg.content).await
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

// ====== Matrix Types ======

#[derive(Debug, Deserialize)]
pub struct MatrixSyncResponse {
    pub next_batch: Option<String>,
    pub rooms: Option<MatrixRooms>,
}

#[derive(Debug, Deserialize)]
pub struct MatrixRooms {
    pub join: Option<std::collections::HashMap<String, MatrixJoinedRoom>>,
}

#[derive(Debug, Deserialize)]
pub struct MatrixJoinedRoom {
    pub timeline: Option<MatrixTimeline>,
}

#[derive(Debug, Deserialize)]
pub struct MatrixTimeline {
    pub events: Option<Vec<MatrixEvent>>,
}

#[derive(Debug, Deserialize)]
pub struct MatrixEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub sender: Option<String>,
    pub content: Option<MatrixMessageContent>,
    pub event_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MatrixMessageContent {
    pub msgtype: Option<String>,
    pub body: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_name() {
        let (tx, _rx) = mpsc::channel(1);
        let ch = MatrixChannel::new(MatrixConfig::default(), tx);
        assert_eq!(ch.name(), "matrix");
    }

    #[test]
    fn test_parse_sync_response() {
        let body = r#"{
            "next_batch": "s12345_67890",
            "rooms": {
                "join": {
                    "!roomid:example.com": {
                        "timeline": {
                            "events": [{
                                "type": "m.room.message",
                                "sender": "@user:example.com",
                                "content": {
                                    "msgtype": "m.text",
                                    "body": "Hello from Matrix!"
                                },
                                "event_id": "$event123"
                            }]
                        }
                    }
                }
            }
        }"#;

        let sync = MatrixChannel::parse_sync_response(body).unwrap();
        assert_eq!(sync.next_batch.as_deref(), Some("s12345_67890"));
        let rooms = sync.rooms.unwrap();
        let join = rooms.join.unwrap();
        let room = join.get("!roomid:example.com").unwrap();
        let events = room.timeline.as_ref().unwrap().events.as_ref().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "m.room.message");
        assert_eq!(events[0].sender.as_deref(), Some("@user:example.com"));
        assert_eq!(
            events[0]
                .content
                .as_ref()
                .unwrap()
                .body
                .as_deref(),
            Some("Hello from Matrix!")
        );
    }

    #[test]
    fn test_parse_sync_response_empty() {
        let body = r#"{"next_batch": "s999"}"#;
        let sync = MatrixChannel::parse_sync_response(body).unwrap();
        assert_eq!(sync.next_batch.as_deref(), Some("s999"));
        assert!(sync.rooms.is_none());
    }

    #[test]
    fn test_parse_sync_response_no_events() {
        let body = r#"{
            "next_batch": "s100",
            "rooms": {
                "join": {
                    "!room:example.com": {
                        "timeline": {
                            "events": []
                        }
                    }
                }
            }
        }"#;

        let sync = MatrixChannel::parse_sync_response(body).unwrap();
        let rooms = sync.rooms.unwrap();
        let join = rooms.join.unwrap();
        let room = join.get("!room:example.com").unwrap();
        assert!(room
            .timeline
            .as_ref()
            .unwrap()
            .events
            .as_ref()
            .unwrap()
            .is_empty());
    }
}
