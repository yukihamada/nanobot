use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Message role in a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::System => write!(f, "system"),
            Role::User => write!(f, "user"),
            Role::Assistant => write!(f, "assistant"),
            Role::Tool => write!(f, "tool"),
        }
    }
}

/// Reason the LLM stopped generating.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    ToolCalls,
    Length,
    Error,
}

/// A tool call request from the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: HashMap<String, serde_json::Value>,
}

/// A single message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: Some(content.into()),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: Some(content.into()),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: Some(content.into()),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn assistant_with_tool_calls(
        content: Option<String>,
        tool_calls: Vec<serde_json::Value>,
    ) -> Self {
        Self {
            role: Role::Assistant,
            content,
            name: None,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, name: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: Some(content.into()),
            name: Some(name.into()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}

/// Request to an LLM provider.
#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub messages: Vec<Message>,
    pub tools: Option<Vec<serde_json::Value>>,
    pub model: String,
    pub max_tokens: u32,
    pub temperature: f64,
}

/// Response from an LLM provider.
#[derive(Debug, Clone)]
pub struct CompletionResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: FinishReason,
    pub usage: TokenUsage,
}

impl CompletionResponse {
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }
}

/// Token usage information.
#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// Message received from a chat channel.
#[derive(Debug, Clone)]
pub struct InboundMessage {
    pub channel: String,
    pub sender_id: String,
    pub chat_id: String,
    pub content: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub media: Vec<String>,
    pub metadata: HashMap<String, serde_json::Value>,
}

impl InboundMessage {
    pub fn new(
        channel: impl Into<String>,
        sender_id: impl Into<String>,
        chat_id: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            channel: channel.into(),
            sender_id: sender_id.into(),
            chat_id: chat_id.into(),
            content: content.into(),
            timestamp: chrono::Utc::now(),
            media: Vec::new(),
            metadata: HashMap::new(),
        }
    }

    pub fn session_key(&self) -> String {
        format!("{}:{}", self.channel, self.chat_id)
    }
}

/// Message to send to a chat channel.
#[derive(Debug, Clone)]
pub struct OutboundMessage {
    pub channel: String,
    pub chat_id: String,
    pub content: String,
    pub reply_to: Option<String>,
    pub media: Vec<String>,
    pub metadata: HashMap<String, serde_json::Value>,
}

impl OutboundMessage {
    pub fn new(
        channel: impl Into<String>,
        chat_id: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            channel: channel.into(),
            chat_id: chat_id.into(),
            content: content.into(),
            reply_to: None,
            media: Vec::new(),
            metadata: HashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_role_display() {
        assert_eq!(Role::System.to_string(), "system");
        assert_eq!(Role::User.to_string(), "user");
        assert_eq!(Role::Assistant.to_string(), "assistant");
        assert_eq!(Role::Tool.to_string(), "tool");
    }

    #[test]
    fn test_role_serde() {
        let json = serde_json::to_string(&Role::Assistant).unwrap();
        assert_eq!(json, "\"assistant\"");
        let role: Role = serde_json::from_str("\"user\"").unwrap();
        assert_eq!(role, Role::User);
    }

    #[test]
    fn test_finish_reason_serde() {
        let json = serde_json::to_string(&FinishReason::ToolCalls).unwrap();
        assert_eq!(json, "\"tool_calls\"");
        let fr: FinishReason = serde_json::from_str("\"stop\"").unwrap();
        assert_eq!(fr, FinishReason::Stop);
    }

    #[test]
    fn test_message_constructors() {
        let sys = Message::system("You are helpful");
        assert_eq!(sys.role, Role::System);
        assert_eq!(sys.content.as_deref(), Some("You are helpful"));

        let user = Message::user("Hello");
        assert_eq!(user.role, Role::User);

        let asst = Message::assistant("Hi there!");
        assert_eq!(asst.role, Role::Assistant);
        assert!(asst.tool_calls.is_none());

        let tool = Message::tool_result("call_1", "read_file", "file contents");
        assert_eq!(tool.role, Role::Tool);
        assert_eq!(tool.tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(tool.name.as_deref(), Some("read_file"));
    }

    #[test]
    fn test_message_with_tool_calls() {
        let calls = vec![serde_json::json!({"id": "1", "type": "function"})];
        let msg = Message::assistant_with_tool_calls(Some("thinking...".into()), calls.clone());
        assert_eq!(msg.tool_calls.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn test_completion_response_has_tool_calls() {
        let resp = CompletionResponse {
            content: Some("hello".into()),
            tool_calls: vec![],
            finish_reason: FinishReason::Stop,
            usage: TokenUsage::default(),
        };
        assert!(!resp.has_tool_calls());

        let resp2 = CompletionResponse {
            content: None,
            tool_calls: vec![ToolCall {
                id: "1".into(),
                name: "test".into(),
                arguments: HashMap::new(),
            }],
            finish_reason: FinishReason::ToolCalls,
            usage: TokenUsage::default(),
        };
        assert!(resp2.has_tool_calls());
    }

    #[test]
    fn test_inbound_message_session_key() {
        let msg = InboundMessage::new("telegram", "user123", "chat456", "hello");
        assert_eq!(msg.session_key(), "telegram:chat456");
        assert_eq!(msg.channel, "telegram");
        assert_eq!(msg.content, "hello");
    }

    #[test]
    fn test_outbound_message() {
        let msg = OutboundMessage::new("discord", "chan1", "response text");
        assert_eq!(msg.channel, "discord");
        assert_eq!(msg.chat_id, "chan1");
        assert_eq!(msg.content, "response text");
        assert!(msg.reply_to.is_none());
        assert!(msg.media.is_empty());
    }
}
