pub mod store;
pub mod file_store;

#[cfg(feature = "dynamodb-backend")]
pub mod dynamo_store;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A single message in session history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// Session metadata stored as the first line of JSONL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionMetadata {
    #[serde(rename = "_type")]
    pub type_field: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// A conversation session.
pub struct Session {
    pub key: String,
    pub messages: Vec<SessionMessage>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub metadata: HashMap<String, serde_json::Value>,
}

impl Session {
    pub fn new(key: impl Into<String>) -> Self {
        let now = chrono::Utc::now();
        Self {
            key: key.into(),
            messages: Vec::new(),
            created_at: now,
            updated_at: now,
            metadata: HashMap::new(),
        }
    }

    /// Add a message to the session.
    pub fn add_message(&mut self, role: &str, content: &str) {
        self.messages.push(SessionMessage {
            role: role.to_string(),
            content: content.to_string(),
            timestamp: Some(crate::util::timestamp()),
            extra: HashMap::new(),
        });
        self.updated_at = chrono::Utc::now();
    }

    /// Add a message with channel source tracking.
    pub fn add_message_from_channel(&mut self, role: &str, content: &str, channel: &str) {
        let mut extra = HashMap::new();
        extra.insert("channel".to_string(), serde_json::json!(channel));
        self.messages.push(SessionMessage {
            role: role.to_string(),
            content: content.to_string(),
            timestamp: Some(crate::util::timestamp()),
            extra,
        });
        self.updated_at = chrono::Utc::now();
    }

    /// Get message history for LLM context (just role + content).
    pub fn get_history(&self, max_messages: usize) -> Vec<serde_json::Value> {
        let start = self.messages.len().saturating_sub(max_messages);
        self.messages[start..]
            .iter()
            .map(|m| {
                serde_json::json!({
                    "role": m.role,
                    "content": m.content,
                })
            })
            .collect()
    }

    /// Get full message history including channel and timestamp (for API responses).
    pub fn get_full_history(&self, max_messages: usize) -> Vec<serde_json::Value> {
        let start = self.messages.len().saturating_sub(max_messages);
        self.messages[start..]
            .iter()
            .map(|m| {
                let mut v = serde_json::json!({
                    "role": m.role,
                    "content": m.content,
                });
                if let Some(ts) = &m.timestamp {
                    v["timestamp"] = serde_json::json!(ts);
                }
                if let Some(ch) = m.extra.get("channel") {
                    v["channel"] = ch.clone();
                }
                v
            })
            .collect()
    }

    /// Clear all messages.
    pub fn clear(&mut self) {
        self.messages.clear();
        self.updated_at = chrono::Utc::now();
    }
}

// Re-export for backward compat: SessionManager is now FileSessionStore
pub use file_store::FileSessionStore as SessionManager;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_new() {
        let session = Session::new("test:123");
        assert_eq!(session.key, "test:123");
        assert!(session.messages.is_empty());
    }

    #[test]
    fn test_session_add_message() {
        let mut session = Session::new("test");
        session.add_message("user", "Hello");
        session.add_message("assistant", "Hi there!");

        assert_eq!(session.messages.len(), 2);
        assert_eq!(session.messages[0].role, "user");
        assert_eq!(session.messages[0].content, "Hello");
        assert_eq!(session.messages[1].role, "assistant");
    }

    #[test]
    fn test_session_get_history() {
        let mut session = Session::new("test");
        for i in 0..10 {
            session.add_message("user", &format!("msg {i}"));
        }

        let history = session.get_history(3);
        assert_eq!(history.len(), 3);
        assert_eq!(history[0]["content"], "msg 7");
        assert_eq!(history[2]["content"], "msg 9");
    }

    #[test]
    fn test_session_clear() {
        let mut session = Session::new("test");
        session.add_message("user", "Hello");
        assert_eq!(session.messages.len(), 1);
        session.clear();
        assert!(session.messages.is_empty());
    }
}
