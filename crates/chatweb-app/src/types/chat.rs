use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: String,
    pub model: Option<String>,
    #[serde(default)]
    pub tools_used: Vec<ToolStep>,
    pub credits_used: Option<f64>,
    pub timestamp: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolStep {
    pub tool: String,
    pub status: ToolStatus,
    pub args_preview: Option<String>,
    pub result: Option<String>,
    pub duration_ms: Option<u64>,
    pub is_error: bool,
    pub iteration: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ToolStatus {
    Running,
    Done,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatRequest {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub session_id: String,
    pub updated_at: String,
    pub message_count: u32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type")]
pub enum SseEvent {
    #[serde(rename = "start")]
    Start {
        #[allow(dead_code)]
        agent: Option<String>,
        #[allow(dead_code)]
        estimated_seconds: Option<u32>,
        session_id: Option<String>,
    },
    #[serde(rename = "tool_start")]
    ToolStart {
        tool: String,
        iteration: Option<u32>,
        max_iter: Option<u32>,
        args_preview: Option<String>,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool: String,
        result: Option<String>,
        iteration: Option<u32>,
        duration_ms: Option<u64>,
        is_error: Option<bool>,
        #[allow(dead_code)]
        is_no_results: Option<bool>,
    },
    #[serde(rename = "thinking")]
    Thinking {
        iteration: Option<u32>,
        max_iter: Option<u32>,
        #[allow(dead_code)]
        tool_count: Option<u32>,
    },
    #[serde(rename = "content_chunk")]
    ContentChunk { text: String },
    #[serde(rename = "content")]
    Content {
        content: Option<String>,
        model_used: Option<String>,
        #[allow(dead_code)]
        tools_used: Option<Vec<String>>,
        credits_remaining: Option<f64>,
        total_credits_used: Option<f64>,
    },
    #[serde(rename = "error")]
    Error {
        content: Option<String>,
        #[allow(dead_code)]
        action: Option<String>,
        #[allow(dead_code)]
        auto_retry_after: Option<u32>,
    },
    #[serde(rename = "done")]
    Done {},
}

/// Tool icon lookup
pub fn tool_icon(name: &str) -> &'static str {
    match name {
        "web_search" => "\u{1f50d}",
        "web_fetch" => "\u{1f310}",
        "code_execute" => "\u{1f4bb}",
        "file_read" => "\u{1f4c4}",
        "file_write" => "\u{270f}\u{fe0f}",
        "file_list" => "\u{1f4c1}",
        "calculator" => "\u{1f9ee}",
        "weather" => "\u{1f324}\u{fe0f}",
        "image_generate" => "\u{1f3a8}",
        "qr_code" => "\u{1f4f1}",
        "wikipedia" => "\u{1f4da}",
        "music_generate" => "\u{1f3b5}",
        "datetime" => "\u{1f550}",
        "translate" => "\u{1f30f}",
        "github_search" | "github_create_issue" | "github_create_pr" => "\u{1f419}",
        "gmail_send" | "gmail_read" => "\u{1f4e7}",
        "calendar_create" | "calendar_list" => "\u{1f4c5}",
        "phone_call" => "\u{1f4de}",
        "slack_send" => "\u{1f4ac}",
        "improve_project" => "\u{1f527}",
        _ => "\u{2699}\u{fe0f}",
    }
}

/// Tool label (Japanese)
pub fn tool_label(name: &str) -> &'static str {
    match name {
        "web_search" => "Web検索",
        "web_fetch" => "ページ取得",
        "code_execute" => "コード実行",
        "file_read" => "ファイル読み込み",
        "file_write" => "ファイル書き込み",
        "file_list" => "ファイル一覧",
        "calculator" => "計算",
        "weather" => "天気",
        "image_generate" => "画像生成",
        "qr_code" => "QRコード",
        "wikipedia" => "Wikipedia",
        "music_generate" => "音楽生成",
        "datetime" => "日時",
        "translate" => "翻訳",
        "github_search" => "GitHub検索",
        "github_create_issue" => "Issue作成",
        "github_create_pr" => "PR作成",
        "gmail_send" => "メール送信",
        "gmail_read" => "メール読み込み",
        "calendar_create" => "予定作成",
        "calendar_list" => "予定一覧",
        "phone_call" => "電話",
        "slack_send" => "Slack送信",
        "improve_project" => "プロジェクト改善",
        _ => "ツール",
    }
}
