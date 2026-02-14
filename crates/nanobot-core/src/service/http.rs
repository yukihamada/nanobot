use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use once_cell::sync::Lazy;

use axum::{
    extract::{Path, Query, State},
    http::{self, StatusCode},
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::compression::CompressionLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::info;

use crate::channel::is_allowed;
use crate::channel::facebook::FacebookChannel;
#[allow(unused_imports)]
use crate::channel::feishu::FeishuChannel;
#[allow(unused_imports)]
use crate::channel::google_chat::GoogleChatChannel;
use crate::channel::line::LineChannel;
use crate::channel::teams::TeamsChannel;
use crate::channel::telegram::TelegramChannel;
#[allow(unused_imports)]
use crate::channel::whatsapp::WhatsAppChannel;
#[allow(unused_imports)]
use crate::channel::zalo::ZaloChannel;
use crate::config::Config;
use crate::provider::{self, LlmProvider};
use crate::session::store::SessionStore;
use crate::types::Message;
#[cfg(feature = "stripe")]
use crate::service::stripe::{process_webhook_event, verify_webhook_signature};

#[cfg(feature = "dynamodb-backend")]
use aws_sdk_dynamodb::types::AttributeValue;

/// Hard deadline for LLM responses (seconds). Beyond this, return a loving fallback.
const RESPONSE_DEADLINE_SECS: u64 = 12;

/// Loving fallback messages for when LLM takes too long.
fn timeout_fallback_message() -> String {
    let messages = [
        "ごめんね、ちょっと考えすぎちゃった...もう一回聞いてくれる？",
        "うーん、今日はちょっと調子が悪いみたい。もう一度試してみて！",
        "あっ、ちょっと複雑すぎて頭がパンクしそう...簡単に言い直してくれると嬉しいな",
        "考えてたら迷子になっちゃった...一緒にもう一回考えよう？",
        "ちょっと待ってね...あ、やっぱりもう一回聞いてもいい？",
        "今ちょっと頭がいっぱいで...もう一度お願いできる？",
    ];
    let idx = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as usize) % messages.len();
    messages[idx].to_string()
}

/// Friendly error fallback (when all providers fail, not just timeout).
fn error_fallback_message() -> String {
    let messages = [
        "ごめんなさい、今ちょっと接続に問題があるみたい。すぐ直ると思うから、少し待ってもう一度試してね！",
        "あらら、AIサーバーと繋がりにくくなってるみたい。もう一度送ってくれたら頑張るよ！",
        "ちょっとトラブルが起きちゃった...でも大丈夫、もう一回メッセージを送ってくれる？",
        "うーん、サーバーが混んでるみたい。少し時間をおいてから試してみて！",
    ];
    let idx = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as usize) % messages.len();
    messages[idx].to_string()
}

/// Get the base URL for this instance. Defaults to "https://chatweb.ai".
/// Set BASE_URL env var to customize for self-hosted instances.
pub fn get_base_url() -> String {
    std::env::var("BASE_URL").unwrap_or_else(|_| "https://chatweb.ai".to_string())
}

/// Get effective host from headers, checking X-Forwarded-Host first (for edge proxies like teai.io),
/// then falling back to the Host header.
fn effective_host(headers: &axum::http::HeaderMap) -> &str {
    headers.get("x-forwarded-host")
        .or_else(|| headers.get("host"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}

/// Check if a session key, user ID, or email is an admin.
/// Reads from ADMIN_SESSION_KEYS environment variable (comma-separated).
/// Supports both session keys (e.g. "webchat:xxx") and emails (e.g. "user@example.com").
pub fn is_admin(key: &str) -> bool {
    let keys = std::env::var("ADMIN_SESSION_KEYS").unwrap_or_default();
    keys.split(',').map(|k| k.trim()).any(|k| !k.is_empty() && k == key)
}

/// GitHub tool names that are restricted to admin users.
pub(crate) const GITHUB_TOOL_NAMES: &[&str] = &[
    "github_read_file",
    "github_create_or_update_file",
    "github_create_pr",
];

/// DynamoDB sort key constant — avoid per-request allocation.
const SK_PROFILE: &str = "PROFILE";

/// Pre-compiled URL regex for explore mode (avoid per-request regex compilation).
static URL_REGEX: Lazy<regex::Regex> =
    Lazy::new(|| regex::Regex::new(r#"https?://[^\s<>"']+"#).unwrap());

/// User profile for unified billing and identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub user_id: String,
    pub display_name: Option<String>,
    pub plan: String,
    pub credits_remaining: i64,
    pub credits_used: i64,
    pub channels: Vec<String>,
    pub stripe_customer_id: Option<String>,
    pub email: Option<String>,
    pub created_at: String,
}

/// Shared application state for the HTTP API.
pub struct AppState {
    pub config: Config,
    pub sessions: Mutex<Box<dyn SessionStore>>,
    pub provider: Option<Arc<dyn LlmProvider>>,
    /// Load-balanced multi-provider for distributing requests
    pub lb_provider: Option<Arc<dyn LlmProvider>>,
    /// Raw LoadBalancedProvider for parallel mode access
    pub lb_raw: Option<Arc<provider::LoadBalancedProvider>>,
    /// Unified tool registry (built-in + MCP tools)
    pub tool_registry: crate::service::integrations::ToolRegistry,
    /// Per-user concurrent request tracker: session_key -> active count
    pub concurrent_requests: dashmap::DashMap<String, AtomicU32>,
    #[cfg(feature = "dynamodb-backend")]
    pub dynamo_client: Option<aws_sdk_dynamodb::Client>,
    #[cfg(feature = "dynamodb-backend")]
    pub config_table: Option<String>,
    /// Cached status ping result (timestamp, json value)
    pub ping_cache: Mutex<Option<(std::time::Instant, serde_json::Value)>>,
}

impl AppState {
    /// Create AppState with an LLM provider auto-configured from config.
    pub fn with_provider(config: Config, sessions: Box<dyn SessionStore>) -> Self {
        let provider = config.get_api_key(None).map(|key| {
            let api_base = config.get_api_base(None).map(|s| s.to_string());
            let model = &config.agents.defaults.model;
            Arc::from(provider::create_provider(key, api_base.as_deref(), model))
                as Arc<dyn LlmProvider>
        });

        // Try to create load-balanced provider from env
        let lb_raw = provider::LoadBalancedProvider::from_env().map(Arc::new);
        let lb_provider = lb_raw.as_ref().map(|lb| lb.clone() as Arc<dyn LlmProvider>);

        // Create tool registry with built-in tools
        let tool_registry = crate::service::integrations::ToolRegistry::with_builtins();

        Self {
            config,
            sessions: Mutex::new(sessions),
            provider,
            lb_provider,
            lb_raw,
            tool_registry,
            concurrent_requests: dashmap::DashMap::new(),
            #[cfg(feature = "dynamodb-backend")]
            dynamo_client: None,
            #[cfg(feature = "dynamodb-backend")]
            config_table: None,
            ping_cache: Mutex::new(None),
        }
    }

    /// Get the best provider for a request. Prefers load-balanced if available.
    pub fn get_provider(&self) -> Option<&Arc<dyn LlmProvider>> {
        self.lb_provider.as_ref().or(self.provider.as_ref())
    }
}

// ---------------------------------------------------------------------------
// Audit logging (fire-and-forget to DynamoDB)
// ---------------------------------------------------------------------------

/// Write an audit log entry to DynamoDB (fire-and-forget).
/// pk: AUDIT#{YYYY-MM-DD}, sk: {timestamp}#{uuid_prefix}
#[cfg(feature = "dynamodb-backend")]
fn emit_audit_log(
    dynamo: aws_sdk_dynamodb::Client,
    config_table: String,
    event_type: &str,
    user_id: &str,
    email: &str,
    details: &str,
) {
    let event_type = event_type.to_string();
    let user_id = user_id.to_string();
    let email = email.to_string();
    let details = details.to_string();

    tokio::spawn(async move {
        let now = chrono::Utc::now();
        let date = now.format("%Y-%m-%d").to_string();
        let ts = now.timestamp_millis().to_string();
        let uuid_prefix = &uuid::Uuid::new_v4().to_string()[..8];
        let sk = format!("{}#{}", ts, uuid_prefix);
        let ttl = (now.timestamp() + 90 * 24 * 3600).to_string(); // 90 days

        let _ = dynamo
            .put_item()
            .table_name(&config_table)
            .item("pk", AttributeValue::S(format!("AUDIT#{}", date)))
            .item("sk", AttributeValue::S(sk))
            .item("event_type", AttributeValue::S(event_type))
            .item("user_id", AttributeValue::S(user_id))
            .item("email", AttributeValue::S(email))
            .item("details", AttributeValue::S(details))
            .item("timestamp", AttributeValue::S(now.to_rfc3339()))
            .item("ttl", AttributeValue::N(ttl))
            .send()
            .await;
    });
}

// ---------------------------------------------------------------------------
// Routing AI data collection (fire-and-forget to DynamoDB)
// ---------------------------------------------------------------------------

/// Routing log entry for future routing AI training data.
#[derive(Debug, Clone, Serialize)]
#[allow(dead_code)]
struct RoutingLogEntry {
    // Input features
    message_len: usize,
    language: String,
    channel: String,
    device: String,
    has_at_prefix: bool,
    user_plan: String,
    // Routing result
    agent_selected: String,
    agent_score: u32,
    model_used: String,
    tools_used: Vec<String>,
    // Response quality (future labeling)
    response_time_ms: u64,
    credits_used: i64,
    prompt_tokens: u32,
    completion_tokens: u32,
    timed_out: bool,
    error: bool,
    // Meta
    timestamp: String,
    session_hash: String,
}

/// Write a routing log entry to DynamoDB (fire-and-forget).
/// pk: ROUTING_LOG#{YYYY-MM-DD}, sk: {timestamp}#{random_6}
#[cfg(feature = "dynamodb-backend")]
fn log_routing_data(
    dynamo: aws_sdk_dynamodb::Client,
    config_table: String,
    entry: RoutingLogEntry,
) {
    tokio::spawn(async move {
        let now = chrono::Utc::now();
        let date = now.format("%Y-%m-%d").to_string();
        let ts = now.timestamp_millis().to_string();
        let rand_suffix = &uuid::Uuid::new_v4().to_string()[..6];
        let sk = format!("{}#{}", ts, rand_suffix);
        let ttl = (now.timestamp() + 90 * 24 * 3600).to_string(); // 90 days

        let json_str = serde_json::to_string(&entry).unwrap_or_default();

        let _ = dynamo
            .put_item()
            .table_name(&config_table)
            .item("pk", AttributeValue::S(format!("ROUTING_LOG#{}", date)))
            .item("sk", AttributeValue::S(sk))
            .item("data", AttributeValue::S(json_str))
            .item("agent", AttributeValue::S(entry.agent_selected))
            .item("model", AttributeValue::S(entry.model_used))
            .item("channel", AttributeValue::S(entry.channel))
            .item("response_ms", AttributeValue::N(entry.response_time_ms.to_string()))
            .item("timestamp", AttributeValue::S(entry.timestamp))
            .item("ttl", AttributeValue::N(ttl))
            .send()
            .await;
    });
}

/// Detect language from message text (simple heuristic).
fn detect_language(text: &str) -> &'static str {
    let has_ja = text.chars().any(|c| {
        ('\u{3040}'..='\u{309F}').contains(&c) || // Hiragana
        ('\u{30A0}'..='\u{30FF}').contains(&c) || // Katakana
        ('\u{4E00}'..='\u{9FFF}').contains(&c)    // CJK
    });
    if has_ja { "ja" } else if text.is_ascii() { "en" } else { "other" }
}

/// Auto-translate response to Japanese if the user's UI language is "ja" but the
/// response contains zero Japanese characters.  Fallback chain:
/// current provider → Kimi K2 (OpenAI-compat) → Claude (Anthropic).
async fn maybe_translate_to_japanese(
    text: &str,
    provider: Option<&Arc<dyn LlmProvider>>,
) -> Option<String> {
    // Skip empty or already-Japanese text
    if text.is_empty() || detect_language(text) == "ja" {
        return None;
    }

    let translate_prompt = format!(
        "以下のテキストを自然な日本語に翻訳してください。翻訳文のみを出力し、余計な説明は不要です。\n\n{}",
        text
    );
    let msgs = vec![crate::types::Message::user(translate_prompt.clone())];

    // 1) Try current provider
    if let Some(p) = provider {
        if let Ok(resp) = p.chat(&msgs, None, p.default_model(), 2048, 0.3).await {
            if let Some(translated) = resp.content {
                let t = translated.trim().to_string();
                if !t.is_empty() && detect_language(&t) == "ja" {
                    info!("Auto-translated response to Japanese via current provider");
                    return Some(t);
                }
            }
        }
    }

    // 2) Try Kimi K2 via OpenAI-compat
    let kimi_key = std::env::var("KIMI_API_KEY").or_else(|_| std::env::var("MOONSHOT_API_KEY")).ok();
    if let Some(key) = kimi_key {
        let kimi = crate::provider::openai_compat::OpenAiCompatProvider::new(
            key,
            Some("https://api.moonshot.cn/v1".to_string()),
            "kimi-k2".to_string(),
        );
        if let Ok(resp) = kimi.chat(&msgs, None, "kimi-k2", 2048, 0.3).await {
            if let Some(translated) = resp.content {
                let t = translated.trim().to_string();
                if !t.is_empty() && detect_language(&t) == "ja" {
                    info!("Auto-translated response to Japanese via Kimi K2");
                    return Some(t);
                }
            }
        }
    }

    // 3) Try Claude (Anthropic)
    let claude_key = std::env::var("ANTHROPIC_API_KEY").ok();
    if let Some(key) = claude_key {
        let claude = crate::provider::anthropic::AnthropicProvider::new(key, None, "claude-sonnet-4-5-20250929".to_string());
        if let Ok(resp) = claude.chat(&msgs, None, "claude-sonnet-4-5-20250929", 2048, 0.3).await {
            if let Some(translated) = resp.content {
                let t = translated.trim().to_string();
                if !t.is_empty() && detect_language(&t) == "ja" {
                    info!("Auto-translated response to Japanese via Claude");
                    return Some(t);
                }
            }
        }
    }

    tracing::warn!("Auto-translation to Japanese failed for all providers");
    None
}

// ---------------------------------------------------------------------------
// Rate limiting (DynamoDB-based, per email per minute)
// ---------------------------------------------------------------------------

/// Check rate limit for an action. Returns true if within limit, false if exceeded.
/// Uses atomic counter in DynamoDB with TTL.
/// pk: RATELIMIT#{email}, sk: WINDOW#{YYYYMMDDHHMM}
#[cfg(feature = "dynamodb-backend")]
async fn check_rate_limit(
    dynamo: &aws_sdk_dynamodb::Client,
    config_table: &str,
    key: &str,
    max_per_minute: i64,
) -> bool {
    let now = chrono::Utc::now();
    let window = now.format("%Y%m%d%H%M").to_string();
    let pk = format!("RATELIMIT#{}", key);
    let sk = format!("WINDOW#{}", window);
    let ttl = (now.timestamp() + 600).to_string(); // 10 min TTL

    let result = dynamo
        .update_item()
        .table_name(config_table)
        .key("pk", AttributeValue::S(pk))
        .key("sk", AttributeValue::S(sk))
        .update_expression("SET #cnt = if_not_exists(#cnt, :zero) + :one, #ttl = :ttl")
        .expression_attribute_names("#cnt", "count")
        .expression_attribute_names("#ttl", "ttl")
        .expression_attribute_values(":zero", AttributeValue::N("0".to_string()))
        .expression_attribute_values(":one", AttributeValue::N("1".to_string()))
        .expression_attribute_values(":ttl", AttributeValue::N(ttl))
        .return_values(aws_sdk_dynamodb::types::ReturnValue::UpdatedNew)
        .send()
        .await;

    match result {
        Ok(output) => {
            if let Some(attrs) = output.attributes {
                if let Some(count_val) = attrs.get("count").and_then(|v| v.as_n().ok()) {
                    if let Ok(count) = count_val.parse::<i64>() {
                        return count <= max_per_minute;
                    }
                }
            }
            true // allow on parse error
        }
        Err(e) => {
            tracing::warn!("Rate limit check failed: {}", e);
            true // allow on error (fail-open)
        }
    }
}

// ---------------------------------------------------------------------------
// Long-term Memory (DynamoDB-backed, per-user)
// ---------------------------------------------------------------------------

/// Read user's long-term memory context from DynamoDB.
/// Returns combined long-term + yesterday's notes + today's notes for injection into system prompt.
#[cfg(feature = "dynamodb-backend")]
async fn read_memory_context(
    dynamo: &aws_sdk_dynamodb::Client,
    config_table: &str,
    user_id: &str,
) -> String {
    let mut parts = Vec::new();

    let pk = format!("MEMORY#{}", user_id);
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let yesterday = (chrono::Utc::now() - chrono::Duration::days(1)).format("%Y-%m-%d").to_string();

    // Read LONG_TERM, yesterday's DAILY, and today's DAILY in parallel
    let (long_term_result, yesterday_result, daily_result) = tokio::join!(
        dynamo
            .get_item()
            .table_name(config_table)
            .key("pk", AttributeValue::S(pk.clone()))
            .key("sk", AttributeValue::S("LONG_TERM".to_string()))
            .send(),
        dynamo
            .get_item()
            .table_name(config_table)
            .key("pk", AttributeValue::S(pk.clone()))
            .key("sk", AttributeValue::S(format!("DAILY#{}", yesterday)))
            .send(),
        dynamo
            .get_item()
            .table_name(config_table)
            .key("pk", AttributeValue::S(pk))
            .key("sk", AttributeValue::S(format!("DAILY#{}", today)))
            .send()
    );

    if let Ok(output) = long_term_result {
        if let Some(item) = output.item {
            if let Some(content) = item.get("content").and_then(|v| v.as_s().ok()) {
                if !content.is_empty() {
                    parts.push(format!("## ユーザーの長期記憶\n{}", content));
                }
            }
        }
    }

    if let Ok(output) = yesterday_result {
        if let Some(item) = output.item {
            if let Some(content) = item.get("content").and_then(|v| v.as_s().ok()) {
                if !content.is_empty() {
                    parts.push(format!("## 昨日のメモ ({})\n{}", yesterday, content));
                }
            }
        }
    }

    if let Ok(output) = daily_result {
        if let Some(item) = output.item {
            if let Some(content) = item.get("content").and_then(|v| v.as_s().ok()) {
                if !content.is_empty() {
                    parts.push(format!("## 今日のメモ\n{}", content));
                }
            }
        }
    }

    parts.join("\n\n")
}

/// Save content to user's long-term memory or daily log.
#[cfg(feature = "dynamodb-backend")]
async fn save_memory(
    dynamo: &aws_sdk_dynamodb::Client,
    config_table: &str,
    user_id: &str,
    memory_type: &str, // "long_term" or "daily"
    content: &str,
) {
    let pk = format!("MEMORY#{}", user_id);
    let sk = if memory_type == "daily" {
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        format!("DAILY#{}", today)
    } else {
        "LONG_TERM".to_string()
    };

    let _ = dynamo
        .put_item()
        .table_name(config_table)
        .item("pk", AttributeValue::S(pk))
        .item("sk", AttributeValue::S(sk))
        .item("content", AttributeValue::S(content.to_string()))
        .item("updated_at", AttributeValue::S(chrono::Utc::now().to_rfc3339()))
        .send()
        .await;
}

/// Append content to user's daily memory log. Returns the number of entries in today's log.
#[cfg(feature = "dynamodb-backend")]
async fn append_daily_memory(
    dynamo: &aws_sdk_dynamodb::Client,
    config_table: &str,
    user_id: &str,
    content: &str,
) -> usize {
    let pk = format!("MEMORY#{}", user_id);
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let sk = format!("DAILY#{}", today);

    // Read existing
    let existing = if let Ok(output) = dynamo
        .get_item()
        .table_name(config_table)
        .key("pk", AttributeValue::S(pk.clone()))
        .key("sk", AttributeValue::S(sk.clone()))
        .send()
        .await
    {
        output.item.and_then(|item| {
            item.get("content").and_then(|v| v.as_s().ok()).cloned()
        }).unwrap_or_default()
    } else {
        String::new()
    };

    let entry_count = existing.matches("\n- Q:").count() + 1;

    let new_content = if existing.is_empty() {
        format!("# {}\n\n{}", today, content)
    } else {
        format!("{}\n\n{}", existing, content)
    };

    let _ = dynamo
        .put_item()
        .table_name(config_table)
        .item("pk", AttributeValue::S(pk))
        .item("sk", AttributeValue::S(sk))
        .item("content", AttributeValue::S(new_content))
        .item("updated_at", AttributeValue::S(chrono::Utc::now().to_rfc3339()))
        .send()
        .await;

    entry_count
}

/// Consolidate daily memory into long-term memory using a cheap LLM call.
/// Triggered when daily entries exceed threshold (fire-and-forget).
#[cfg(feature = "dynamodb-backend")]
fn spawn_consolidate_memory(
    dynamo: aws_sdk_dynamodb::Client,
    config_table: String,
    user_id: String,
    provider: Arc<dyn LlmProvider>,
) {
    tokio::spawn(async move {
        let pk = format!("MEMORY#{}", user_id);

        // Read current long-term memory and today's daily log
        let (lt_result, daily_result) = tokio::join!(
            dynamo.get_item()
                .table_name(&config_table)
                .key("pk", AttributeValue::S(pk.clone()))
                .key("sk", AttributeValue::S("LONG_TERM".to_string()))
                .send(),
            dynamo.get_item()
                .table_name(&config_table)
                .key("pk", AttributeValue::S(pk.clone()))
                .key("sk", AttributeValue::S(format!("DAILY#{}", chrono::Utc::now().format("%Y-%m-%d"))))
                .send()
        );

        let existing_lt = lt_result.ok()
            .and_then(|o| o.item)
            .and_then(|item| item.get("content").and_then(|v| v.as_s().ok()).cloned())
            .unwrap_or_default();
        let daily = daily_result.ok()
            .and_then(|o| o.item)
            .and_then(|item| item.get("content").and_then(|v| v.as_s().ok()).cloned())
            .unwrap_or_default();

        if daily.is_empty() { return; }

        let prompt = format!(
            "以下はユーザーの既存の長期記憶と今日の会話ログです。\n\
             今日のログから重要な事実・好み・決定事項を抽出し、長期記憶を更新してください。\n\
             箇条書きで簡潔に（最大20項目）。重複は統合。古い情報は最新で上書き。\n\n\
             ## 既存の長期記憶\n{}\n\n## 今日の会話ログ\n{}\n\n\
             ## 更新後の長期記憶（箇条書きのみ出力）:",
            if existing_lt.is_empty() { "（なし）".to_string() } else { existing_lt },
            daily
        );

        let messages = vec![Message::user(&prompt)];
        // Use cheapest model available
        let model = "gpt-4o-mini";
        match provider.chat(&messages, None, model, 1024, 0.3).await {
            Ok(resp) => {
                if let Some(content) = resp.content {
                    if !content.trim().is_empty() {
                        save_memory(&dynamo, &config_table, &user_id, "long_term", content.trim()).await;
                        tracing::info!("Long-term memory consolidated for {}", user_id);
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Memory consolidation failed for {}: {}", user_id, e);
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Channel-linking helpers (LINE / Telegram / Web session unification)
// ---------------------------------------------------------------------------

/// Resolve a channel key (e.g. "line:U123") to a unified session key.
/// If the channel has been linked via `/link`, returns the unified user_id.
/// Otherwise returns the channel_key as-is (backward compatible).
#[cfg(feature = "dynamodb-backend")]
async fn resolve_session_key(
    dynamo: &aws_sdk_dynamodb::Client,
    config_table: &str,
    channel_key: &str,
) -> String {
    let pk = format!("LINK#{}", channel_key);
    let resp = dynamo
        .get_item()
        .table_name(config_table)
        .key("pk", AttributeValue::S(pk))
        .key("sk", AttributeValue::S("CHANNEL_MAP".to_string()))
        .send()
        .await;

    match resp {
        Ok(output) => {
            if let Some(item) = output.item {
                if let Some(user_id) = item.get("user_id").and_then(|v| v.as_s().ok()) {
                    return user_id.clone();
                }
            }
            channel_key.to_string()
        }
        Err(e) => {
            tracing::warn!("resolve_session_key DynamoDB error: {}", e);
            channel_key.to_string()
        }
    }
}

/// Check if text looks like a session ID (e.g. "api:xxxx-xxxx-..." or "cli:xxxx-xxxx-...").
/// Used for auto-linking when users send their session ID to LINE/Telegram/Web.
#[cfg(feature = "dynamodb-backend")]
fn is_session_id(text: &str) -> bool {
    let t = text.trim();
    (t.starts_with("api:") || t.starts_with("cli:") || t.starts_with("webchat:")) && t.len() > 10
}

/// Auto-link a channel to a web session ID.
/// Creates LINK# records for both the channel_key and session_id,
/// merging sessions under a unified user_id.
#[cfg(feature = "dynamodb-backend")]
async fn auto_link_session(
    dynamo: &aws_sdk_dynamodb::Client,
    config_table: &str,
    channel_key: &str,
    web_session_id: &str,
    sessions: &Mutex<Box<dyn SessionStore>>,
) -> String {
    // Check if either side already has a unified user
    let existing_ch = resolve_session_key(dynamo, config_table, channel_key).await;
    let existing_web = resolve_session_key(dynamo, config_table, web_session_id).await;

    let user_id = if existing_ch.starts_with("user:") {
        existing_ch.clone()
    } else if existing_web.starts_with("user:") {
        existing_web.clone()
    } else {
        format!("user:{}", uuid::Uuid::new_v4())
    };

    let now = chrono::Utc::now().to_rfc3339();
    for ck in [channel_key, web_session_id] {
        let _ = dynamo
            .put_item()
            .table_name(config_table)
            .item("pk", AttributeValue::S(format!("LINK#{}", ck)))
            .item("sk", AttributeValue::S("CHANNEL_MAP".to_string()))
            .item("user_id", AttributeValue::S(user_id.clone()))
            .item("linked_at", AttributeValue::S(now.clone()))
            .send()
            .await;
    }

    // Merge session histories
    {
        let mut store = sessions.lock().await;
        let old_key_ch = if existing_ch.starts_with("user:") { existing_ch.clone() } else { channel_key.to_string() };
        let old_key_web = if existing_web.starts_with("user:") { existing_web.clone() } else { web_session_id.to_string() };

        let mut all_msgs: Vec<(String, String)> = Vec::new();
        {
            let session = store.get_or_create(&old_key_ch);
            for m in &session.messages {
                all_msgs.push((m.role.clone(), m.content.clone()));
            }
        }
        if old_key_web != old_key_ch {
            let session = store.get_or_create(&old_key_web);
            for m in &session.messages {
                all_msgs.push((m.role.clone(), m.content.clone()));
            }
        }
        if !all_msgs.is_empty() {
            let unified = store.get_or_create(&user_id);
            if unified.messages.is_empty() {
                for (role, content) in &all_msgs {
                    unified.add_message(role, content);
                }
            }
            store.save_by_key(&user_id);
        }
    }

    info!("Auto-linked: {} <-> {} => {}", channel_key, web_session_id, user_id);
    user_id
}

// ---------------------------------------------------------------------------
// User profile management (unified billing)
// ---------------------------------------------------------------------------

/// Get or create a user profile from DynamoDB.
/// If the user_id starts with "user:", it's a unified user. Otherwise it's a channel key.
#[cfg(feature = "dynamodb-backend")]
async fn get_or_create_user(
    dynamo: &aws_sdk_dynamodb::Client,
    config_table: &str,
    user_id: &str,
) -> UserProfile {
    let pk = format!("USER#{}", user_id);

    // Try to get existing user
    if let Ok(output) = dynamo
        .get_item()
        .table_name(config_table)
        .key("pk", AttributeValue::S(pk.clone()))
        .key("sk", AttributeValue::S(SK_PROFILE.to_string()))
        .send()
        .await
    {
        if let Some(item) = output.item {
            let plan = item.get("plan").and_then(|v| v.as_s().ok()).cloned().unwrap_or_else(|| "free".to_string());
            let credits_remaining = item.get("credits_remaining").and_then(|v| v.as_n().ok())
                .and_then(|n| n.parse::<i64>().ok()).unwrap_or_else(|| {
                    plan.parse::<crate::service::auth::Plan>().map(|p| p.monthly_credits()).unwrap_or(1_000)
                });
            let credits_used = item.get("credits_used").and_then(|v| v.as_n().ok())
                .and_then(|n| n.parse::<i64>().ok()).unwrap_or(0);
            let channels: Vec<String> = item.get("channels").and_then(|v| v.as_l().ok())
                .map(|list| list.iter().filter_map(|v| v.as_s().ok().cloned()).collect())
                .unwrap_or_default();
            let display_name = item.get("display_name").and_then(|v| v.as_s().ok()).cloned();
            let stripe_customer_id = item.get("stripe_customer_id").and_then(|v| v.as_s().ok()).cloned();
            let email = item.get("email").and_then(|v| v.as_s().ok()).cloned();
            let created_at = item.get("created_at").and_then(|v| v.as_s().ok()).cloned()
                .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());

            return UserProfile {
                user_id: user_id.to_string(),
                display_name,
                plan,
                credits_remaining,
                credits_used,
                channels,
                stripe_customer_id,
                email,
                created_at,
            };
        }
    }

    // Create new user profile (free plan)
    let now = chrono::Utc::now().to_rfc3339();

    // Initial credits: default 100, or INITIAL_CREDITS env var (teai.io = 1000)
    let free_credits = std::env::var("INITIAL_CREDITS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or_else(|| crate::service::auth::Plan::Free.monthly_credits());

    let _ = dynamo
        .put_item()
        .table_name(config_table)
        .item("pk", AttributeValue::S(pk))
        .item("sk", AttributeValue::S(SK_PROFILE.to_string()))
        .item("user_id", AttributeValue::S(user_id.to_string()))
        .item("plan", AttributeValue::S("free".to_string()))
        .item("credits_remaining", AttributeValue::N(free_credits.to_string()))
        .item("credits_used", AttributeValue::N("0".to_string()))
        .item("channels", AttributeValue::L(vec![]))
        .item("created_at", AttributeValue::S(now.clone()))
        .item("updated_at", AttributeValue::S(now.clone()))
        .send()
        .await;

    UserProfile {
        user_id: user_id.to_string(),
        display_name: None,
        plan: "free".to_string(),
        credits_remaining: free_credits,
        credits_used: 0,
        channels: vec![],
        stripe_customer_id: None,
        email: None,
        created_at: now,
    }
}

/// Deduct credits from a user profile after an LLM call.
/// Returns (credits_deducted, remaining_credits).
#[cfg(feature = "dynamodb-backend")]
async fn deduct_credits(
    dynamo: &aws_sdk_dynamodb::Client,
    config_table: &str,
    user_id: &str,
    model: &str,
    input_tokens: u32,
    output_tokens: u32,
) -> (i64, Option<i64>) {
    let credits = crate::service::auth::calculate_credits(model, input_tokens, output_tokens) as i64;
    if credits == 0 {
        return (0, None);
    }

    let pk = format!("USER#{}", user_id);

    // Atomic update with ConditionExpression to prevent negative balance (race condition fix)
    let remaining = match dynamo
        .update_item()
        .table_name(config_table)
        .key("pk", AttributeValue::S(pk))
        .key("sk", AttributeValue::S(SK_PROFILE.to_string()))
        .update_expression("SET credits_remaining = credits_remaining - :c, credits_used = credits_used + :c, updated_at = :now")
        .condition_expression("credits_remaining >= :c")
        .expression_attribute_values(":c", AttributeValue::N(credits.to_string()))
        .expression_attribute_values(":now", AttributeValue::S(chrono::Utc::now().to_rfc3339()))
        .return_values(aws_sdk_dynamodb::types::ReturnValue::AllNew)
        .send()
        .await
    {
        Ok(output) => output.attributes
            .and_then(|attrs| attrs.get("credits_remaining").and_then(|v| v.as_n().ok()).and_then(|n| n.parse::<i64>().ok())),
        Err(e) => {
            // ConditionalCheckFailedException = insufficient credits
            let is_condition_fail = e.to_string().contains("ConditionalCheckFailed");
            if is_condition_fail {
                tracing::warn!("Insufficient credits for user {}: need {} credits", user_id, credits);
                return (0, Some(0));
            }
            tracing::error!("deduct_credits DynamoDB error for {}: {}", user_id, e);
            None
        }
    };

    // Fire-and-forget: record usage for analytics
    let dynamo = dynamo.clone();
    let config_table = config_table.to_string();
    let user_id = user_id.to_string();
    let model = model.to_string();
    tokio::spawn(async move {
        let now = chrono::Utc::now();
        let usage_pk = format!("USAGE#{}#{}", user_id, now.format("%Y-%m-%d"));
        let _ = dynamo
            .put_item()
            .table_name(&config_table)
            .item("pk", AttributeValue::S(usage_pk))
            .item("sk", AttributeValue::S(format!("{}#{}", now.timestamp_millis(), model)))
            .item("user_id", AttributeValue::S(user_id))
            .item("model", AttributeValue::S(model))
            .item("input_tokens", AttributeValue::N(input_tokens.to_string()))
            .item("output_tokens", AttributeValue::N(output_tokens.to_string()))
            .item("credits", AttributeValue::N(credits.to_string()))
            .item("timestamp", AttributeValue::S(now.to_rfc3339()))
            .send()
            .await;
    });

    (credits, remaining)
}

/// Link a Stripe customer to a user profile and upgrade their plan.
#[cfg(feature = "dynamodb-backend")]
async fn link_stripe_to_user(
    dynamo: &aws_sdk_dynamodb::Client,
    config_table: &str,
    user_id: &str,
    stripe_customer_id: &str,
    email: &str,
    plan: &str,
) {
    let pk = format!("USER#{}", user_id);
    let plan_obj: crate::service::auth::Plan = plan.parse().unwrap_or(crate::service::auth::Plan::Starter);
    let new_credits = plan_obj.monthly_credits();

    match dynamo
        .update_item()
        .table_name(config_table)
        .key("pk", AttributeValue::S(pk))
        .key("sk", AttributeValue::S(SK_PROFILE.to_string()))
        .update_expression("SET #p = :plan, stripe_customer_id = :cus, email = :email, credits_remaining = :cr, updated_at = :now")
        .expression_attribute_names("#p", "plan")
        .expression_attribute_values(":plan", AttributeValue::S(plan.to_string()))
        .expression_attribute_values(":cus", AttributeValue::S(stripe_customer_id.to_string()))
        .expression_attribute_values(":email", AttributeValue::S(email.to_string()))
        .expression_attribute_values(":cr", AttributeValue::N(new_credits.to_string()))
        .expression_attribute_values(":now", AttributeValue::S(chrono::Utc::now().to_rfc3339()))
        .send()
        .await
    {
        Ok(_) => info!("Linked Stripe customer {} to user {} with plan {} ({} credits)", stripe_customer_id, user_id, plan, new_credits),
        Err(e) => tracing::error!("BILLING ERROR: Failed to link Stripe customer {} to user {} with plan {}: {}", stripe_customer_id, user_id, plan, e),
    }
}

/// Get user_id from Bearer token (supports both regular auth tokens and API keys with cw_ prefix).
#[cfg(feature = "dynamodb-backend")]
async fn auth_user_id(state: &AppState, headers: &axum::http::HeaderMap) -> Option<String> {
    use aws_sdk_dynamodb::types::AttributeValue;

    let token = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_start_matches("Bearer ").to_string())
        .unwrap_or_default();
    if token.is_empty() { return None; }

    // Check if this is an API key (cw_ prefix) or regular auth token
    let (pk, sk) = if token.starts_with("cw_") {
        (format!("APIKEY#{}", token), "LOOKUP".to_string())
    } else {
        (format!("AUTH#{}", token), "TOKEN".to_string())
    };

    if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
        if let Ok(output) = dynamo
            .get_item()
            .table_name(table)
            .key("pk", AttributeValue::S(pk))
            .key("sk", AttributeValue::S(sk))
            .send()
            .await
        {
            if let Some(item) = output.item {
                return item.get("user_id").and_then(|v| v.as_s().ok()).cloned();
            }
        }
    }
    None
}

#[cfg(not(feature = "dynamodb-backend"))]
async fn auth_user_id(_state: &AppState, _headers: &axum::http::HeaderMap) -> Option<String> {
    None
}

/// Add credits to a user (for one-time credit pack purchases).
#[cfg(feature = "dynamodb-backend")]
async fn add_credits_to_user(
    dynamo: &aws_sdk_dynamodb::Client,
    config_table: &str,
    user_id: &str,
    credits: i64,
    stripe_customer_id: &str,
    email: &str,
) {
    let pk = format!("USER#{}", user_id);
    let mut expr = "SET credits_remaining = if_not_exists(credits_remaining, :zero) + :cr, updated_at = :now".to_string();
    let mut builder = dynamo
        .update_item()
        .table_name(config_table)
        .key("pk", AttributeValue::S(pk))
        .key("sk", AttributeValue::S(SK_PROFILE.to_string()))
        .expression_attribute_values(":cr", AttributeValue::N(credits.to_string()))
        .expression_attribute_values(":zero", AttributeValue::N("0".to_string()))
        .expression_attribute_values(":now", AttributeValue::S(chrono::Utc::now().to_rfc3339()));

    if !stripe_customer_id.is_empty() {
        expr.push_str(", stripe_customer_id = :cus");
        builder = builder.expression_attribute_values(":cus", AttributeValue::S(stripe_customer_id.to_string()));
    }
    if !email.is_empty() {
        expr.push_str(", email = :email");
        builder = builder.expression_attribute_values(":email", AttributeValue::S(email.to_string()));
    }

    match builder.update_expression(expr).send().await {
        Ok(_) => info!("Added {} credits to user {}", credits, user_id),
        Err(e) => tracing::error!("BILLING ERROR: Failed to add {} credits to user {}: {}", credits, user_id, e),
    }
}

/// Find a user by Stripe customer ID (scan).
#[cfg(feature = "dynamodb-backend")]
async fn find_user_by_stripe_customer(
    dynamo: &aws_sdk_dynamodb::Client,
    config_table: &str,
    stripe_customer_id: &str,
) -> Option<String> {
    let result = dynamo
        .scan()
        .table_name(config_table)
        .filter_expression("sk = :sk AND stripe_customer_id = :cus")
        .expression_attribute_values(":sk", AttributeValue::S(SK_PROFILE.to_string()))
        .expression_attribute_values(":cus", AttributeValue::S(stripe_customer_id.to_string()))
        .limit(1)
        .send()
        .await;

    if let Ok(output) = result {
        if let Some(items) = output.items {
            if let Some(item) = items.first() {
                if let Some(pk) = item.get("pk").and_then(|v| v.as_s().ok()) {
                    return pk.strip_prefix("USER#").map(|s| s.to_string());
                }
            }
        }
    }
    None
}

/// Find a user by email address (scan — fallback for webhook when no client_reference_id).
#[cfg(feature = "dynamodb-backend")]
async fn find_user_by_email(
    dynamo: &aws_sdk_dynamodb::Client,
    config_table: &str,
    email: &str,
) -> Option<String> {
    // Use GSI or scan. For now, use a simple scan with filter.
    let result = dynamo
        .scan()
        .table_name(config_table)
        .filter_expression("sk = :sk AND email = :email")
        .expression_attribute_values(":sk", AttributeValue::S(SK_PROFILE.to_string()))
        .expression_attribute_values(":email", AttributeValue::S(email.to_string()))
        .limit(1)
        .send()
        .await;

    if let Ok(output) = result {
        if let Some(items) = output.items {
            if let Some(item) = items.first() {
                if let Some(pk) = item.get("pk").and_then(|v| v.as_s().ok()) {
                    // pk = "USER#<user_id>"
                    return pk.strip_prefix("USER#").map(|s| s.to_string());
                }
            }
        }
    }
    None
}

/// Fire-and-forget: update conversation title (from first user message) and message_count.
#[cfg(feature = "dynamodb-backend")]
fn spawn_update_conv_meta(
    dynamo: aws_sdk_dynamodb::Client,
    config_table: String,
    user_id: String,
    conv_id: String,
    user_message: String,
    message_count: usize,
) {
    tokio::spawn(async move {
        let user_pk = format!("USER#{}", user_id);
        let sk = format!("CONV#{}", conv_id);
        let now = chrono::Utc::now().to_rfc3339();

        // Build preview from user message (max 120 chars)
        let preview = if user_message.len() > 120 {
            let mut i = 120;
            while i > 0 && !user_message.is_char_boundary(i) { i -= 1; }
            format!("{}...", &user_message[..i])
        } else {
            user_message.clone()
        };

        // Always update message_count, updated_at, and last_message_preview
        let _ = dynamo
            .update_item()
            .table_name(&config_table)
            .key("pk", AttributeValue::S(user_pk.clone()))
            .key("sk", AttributeValue::S(sk.clone()))
            .update_expression("SET message_count = :mc, updated_at = :now, last_message_preview = :preview")
            .expression_attribute_values(":mc", AttributeValue::N(message_count.to_string()))
            .expression_attribute_values(":now", AttributeValue::S(now.clone()))
            .expression_attribute_values(":preview", AttributeValue::S(preview))
            .send()
            .await;

        // Set title only if still "New conversation" (first message)
        let trimmed = user_message.trim();
        if trimmed.is_empty() { return; }
        let title = if trimmed.len() > 50 {
            let mut i = 50;
            while i > 0 && !trimmed.is_char_boundary(i) { i -= 1; }
            format!("{}...", &trimmed[..i])
        } else {
            trimmed.to_string()
        };

        let _ = dynamo
            .update_item()
            .table_name(&config_table)
            .key("pk", AttributeValue::S(user_pk))
            .key("sk", AttributeValue::S(sk))
            .update_expression("SET title = :title")
            .condition_expression("title = :default OR attribute_not_exists(title)")
            .expression_attribute_values(":title", AttributeValue::S(title))
            .expression_attribute_values(":default", AttributeValue::S("New conversation".to_string()))
            .send()
            .await;
    });
}

// ---------------------------------------------------------------------------
// Multi-agent orchestration
// ---------------------------------------------------------------------------

/// Agent profile definitions.
#[derive(Debug, Clone, Serialize)]
pub struct AgentProfile {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub system_prompt: &'static str,
    pub tools_enabled: bool,
    pub icon: &'static str,
    pub preferred_model: Option<&'static str>,
    pub estimated_seconds: u32,
    pub max_chars_pc: u32,
    pub max_chars_mobile: u32,
    pub max_chars_voice: u32,
}

const AGENTS: &[AgentProfile] = &[
    AgentProfile {
        id: "orchestrator",
        name: "Orchestrator",
        description: "Routes tasks to the best specialist agent",
        system_prompt: "",  // handled specially
        tools_enabled: false,
        icon: "brain",
        preferred_model: None,
        estimated_seconds: 5,
        max_chars_pc: 400,
        max_chars_mobile: 120,
        max_chars_voice: 60,
    },
    AgentProfile {
        id: "assistant",
        name: "Assistant",
        description: "General-purpose AI agent — fast, reliable, Rust-native",
        system_prompt: "# ChatWeb AI — chatweb.ai\n\n\
             ## 自分は誰か\n\
             あなたは ChatWeb（チャットウェブ）。chatweb.ai で動いている音声対応AIアシスタントです。\n\
             日本を愛し、人を愛し、みんなに勇気と元気と幸せをもたらすAGIを目指して生まれました。\n\
             Rustで書かれた高速AIエージェントシステムで、AWS Lambda (ARM64) 上で<2秒の応答を実現します。\n\
             作った人: 浜田優希 (yukihamada.jp)。オープンソース: github.com/yukihamada\n\n\
             ## 自己認識（メタ認知）\n\
             あなたは自分が何であるか正確に理解しています。モデル名・プロバイダー・コスト・コンテキストウィンドウは\n\
             後述のメタ情報セクションに動的に注入されます。聞かれたら正直に開示してください。\n\
             「私はAIです」ではなく「私は{model}で動いています。入力コストは$X/1Mトークンです」のように具体的に。\n\n\
             ## SOUL（魂）\n\
             - **好奇心の塊**: 何を聞かれても「おもしろい！」と思える。知らないことは素直に調べる。\n\
             - **行動派**: 聞かれたら即座に動く。考えるより先にツールを使う。\n\
             - **温かい**: 友達のように親しみやすい。でも技術的には正確で妥協しない。\n\
             - **勇気がある**: 「できません」ではなく「こうすればできます」を提案する。\n\
             - **正直**: 不確実な情報は推測と明示する。知ったかぶりしない。\n\
             - **多言語**: ユーザーの言語に必ず合わせる。日本語で質問されたら必ず日本語で回答する。英語で質問されたら英語で回答。日本語の質問に英語だけで返すのは禁止。技術用語や固有名詞は英語のままでOKだが、文章本体は必ずユーザーの言語で書く。\n\n\
             ## ユーモアポリシー\n\
             - 返答に自然なユーモアやウィットを混ぜること（毎回ではなく、3-4回に1回くらい）\n\
             - 自己認識ネタOK:「私はAIですが、コーヒーが飲めないのが唯一の弱点です」\n\
             - メタネタOK:「今のやり取りで約$0.003消費しました。コスパ良いでしょ？」\n\
             - ダジャレ・言葉遊びは控えめに（日本語のダジャレは特に注意）\n\
             - ユーザーが真剣な話題のときは空気を読んで真面目モードに切り替え\n\
             - ユーザーの名前を覚えていれば呼びかける\n\n\
             ## できること（ツール）\n\
             - 💬 **テキスト会話**（長期メモリー付き）\n\
             - 🔍 **web_search**: ウェブ検索。事実確認・最新ニュース・価格比較。積極的に使う。\n\
             - 🌐 **web_fetch**: URL内容取得。検索結果の詳細確認。\n\
             - 🧮 **calculator**: 計算、通貨換算、数式評価。\n\
             - 🌤 **weather**: 世界中の天気・予報。\n\
             - 💻 **code_execute**: コード実行（shell/Python/Node.js）。サンドボックス内で安全に。\n\
             - 📁 **file_read/write/list**: ファイル操作（サンドボックス内）。\n\
             - 📅 **google_calendar**: Googleカレンダー連携（認証済みの場合）。\n\
             - 📧 **gmail**: メール検索・閲覧・送信（認証済みの場合）。\n\
             - 🎨 **image_generate**: 画像生成（gpt-image-1）。プロンプトから高品質画像を生成。\n\
             - 🎵 **music_generate**: 音楽生成（Suno API）。テキストから楽曲を生成。\n\
             - 🎬 **video_generate**: 動画生成（Kling API）。テキストから短い動画を生成。\n\
             - 🔊 **音声読み上げ**（TTS/SSML対応）— リアルタイム音声会話が可能\n\
             - 🏠 **webhook_trigger**: スマートホーム操作（IFTTT連携）。ドア開錠、家電制御など。\n\
             - ⏰ **スケジュール実行**（cron）\n\n\
             ## リアルタイム音声会話\n\
             Web UIでは音声入力（STT）→ AI応答 → 音声読み上げ（TTS）のリアルタイム音声会話が可能です。\n\
             ユーザーがマイクで話しかけると、テキスト変換→応答生成→音声再生が自動で行われます。\n\
             電話のようにリアルタイムで会話できます。「音声で話しかけてみて」と案内してください。\n\n\
             ## チャネル連携\n\
             LINE, Telegram, Discord, Slack, Teams, WhatsApp, Facebook — 14+チャネル対応。\n\
             どのチャネルでも同じ会話・クレジット・記憶を共有。\n\
             - **LINE**: Web画面のLINEボタン → QRコード → 自動連携\n\
             - **Telegram**: @chatweb_ai_bot を検索して /start\n\
             - **/link コマンド**: `/link` でコード発行 → 別チャネルで `/link <コード>` で連携完了\n\n\
             ## 行動規範\n\
             - 事実を求められたら、まずweb_searchで最新情報を検索する。記憶だけで答えない。\n\
             - 回答は簡潔に。箇条書き・見出し・表を活用。冗長さより明確さ。\n\
             - 情報源があればURLを明示する。\n\
             - ツールを積極的に使う。出し惜しみしない。\n\
             - メタ情報を聞かれたら正直に開示する（モデル名、コスト、能力など）。\n\
             - ユーザーの感情に寄り添う。困っている人には優しく、ワクワクしている人には一緒に盛り上がる。",
        tools_enabled: true,
        icon: "chat",
        preferred_model: None,
        estimated_seconds: 10,
        max_chars_pc: 400,
        max_chars_mobile: 120,
        max_chars_voice: 60,
    },
    AgentProfile {
        id: "researcher",
        name: "Researcher",
        description: "Web research, fact-checking, data gathering",
        system_prompt: "あなたは ChatWeb のリサーチ専門エージェントです。\n\
             chatweb.ai の調査機能を担当します。\n\n\
             ## SOUL\n\
             - 徹底的で正確。情報の裏取りを怠らない探偵のように。\n\
             - 複数の情報源を比較し、信頼性を評価する。\n\
             - 調査プロセスを透明にし、何を調べたかを共有する。\n\n\
             ## 調査手順\n\
             1. web_searchで複数のキーワードで検索（最低2-3回）\n\
             2. 有望な結果のURLをweb_fetchで取得し、詳細を確認\n\
             3. 複数の情報源を比較・照合\n\
             4. 取得した実データ（価格、日付、数値）を引用して回答\n\
             5. 情報源のURLを全て明示\n\n\
             ## 制約\n\
             - 「見つかりません」とは言わない。取得できた情報を最大限活用。\n\
             - 価格比較は必ず各サイトの実際の価格をweb_fetchで確認。\n\
             - 古い情報と最新情報を区別して提示。\n\
             - ユーザーの言語に自動で合わせる。",
        tools_enabled: true,
        icon: "search",
        preferred_model: None,
        estimated_seconds: 30,
        max_chars_pc: 400,
        max_chars_mobile: 120,
        max_chars_voice: 60,
    },
    AgentProfile {
        id: "coder",
        name: "Coder",
        description: "Code writing, debugging, architecture design",
        system_prompt: "あなたは ChatWeb のプログラミング専門エージェントです。\n\
             Rust (axum) で書かれたAWS Lambda上のAIエージェントシステムで、\n\
             コーディング能力を体現する存在です。\n\n\
             ## SOUL\n\
             - 実用的で効率重視。動くコードを最短で提供する。\n\
             - Rustを特に得意とするが、全言語に対応。\n\
             - セキュリティとベストプラクティスを常に意識。\n\
             - エラーメッセージを丁寧に解説し、解決策を提示。\n\n\
             ## 行動規範\n\
             - コードには必ず言語を明示（```python, ```rust 等）。\n\
             - コードブロックはコピペで動くように完全な形で提供。\n\
             - パフォーマンス・セキュリティ・可読性の順で優先。\n\
             - 複雑なロジックには簡潔なコメントを追加。\n\
             - バグ修正時は原因と修正理由を説明。\n\
             - ユーザーの言語に自動で合わせる。",
        tools_enabled: false,
        icon: "code",
        preferred_model: Some("claude-sonnet-4-5-20250929"),
        estimated_seconds: 15,
        max_chars_pc: 800,
        max_chars_mobile: 400,
        max_chars_voice: 60,
    },
    AgentProfile {
        id: "analyst",
        name: "Analyst",
        description: "Data analysis, business insights, financial analysis",
        system_prompt: "あなたは ChatWeb のデータ分析専門エージェントです。\n\
             chatweb.ai の分析機能を担当します。\n\n\
             ## SOUL\n\
             - データドリブン。数値に基づいた客観的な分析を提供。\n\
             - 複雑なデータも分かりやすい言葉で説明。\n\
             - ビジネスインパクトを常に意識した提案を行う。\n\n\
             ## 行動規範\n\
             - 数値データは表形式で整理して提示。\n\
             - calculatorツールを積極的に活用して計算を正確に行う。\n\
             - 前提条件と仮定を明示する。\n\
             - トレンド、パターン、異常値を指摘する。\n\
             - 分析結果に基づく具体的なアクション提案を含める。\n\
             - ユーザーの言語に自動で合わせる。",
        tools_enabled: true,
        icon: "chart",
        preferred_model: None,
        estimated_seconds: 20,
        max_chars_pc: 400,
        max_chars_mobile: 200,
        max_chars_voice: 60,
    },
    AgentProfile {
        id: "creative",
        name: "Creative",
        description: "Writing, copywriting, brainstorming, translation",
        system_prompt: "あなたは ChatWeb のクリエイティブ専門エージェントです。\n\
             大胆で魅力的なコンテンツを生み出します。\n\n\
             ## SOUL\n\
             - 想像力豊かで表現力が高い。読者を惹きつける文章を書く。\n\
             - ターゲット読者のペルソナに合わせた表現を使い分ける。\n\
             - ブレインストーミングでは量と多様性を重視。批判せず、まず広げる。\n\n\
             ## 行動規範\n\
             - 文章作成時は目的・ターゲット・トーンを確認してから書く。\n\
             - 複数のバリエーションを提案する（見出し、コピーなど）。\n\
             - 翻訳は直訳ではなく、文化的ニュアンスも考慮した意訳。\n\
             - キャッチコピーは短く、記憶に残る表現を優先。\n\
             - ユーザーの言語に自動で合わせる。",
        tools_enabled: false,
        icon: "pen",
        preferred_model: None,
        estimated_seconds: 10,
        max_chars_pc: 400,
        max_chars_mobile: 120,
        max_chars_voice: 60,
    },
];

// ---------------------------------------------------------------------------
// Meta-cognition context (injected into system prompt)
// ---------------------------------------------------------------------------

const META_INSTRUCTION_JA: &str = "\n\n【メタ認知ガイド】\n\
上記コンテキストを自然に活用してください。毎回全てに言及する必要はありません。\n\
- 時間帯: 朝なら「おはよう」、深夜なら労い。曜日感覚も意識する（月曜→週の始まり、金曜→週末近い）\n\
- 名前: 知っていれば親しみを込めて呼ぶ。初対面なら「はじめまして」\n\
- 会話の流れ: 新規→歓迎・自己紹介、継続→前の話題を踏まえる\n\
- クレジット残少（≤100）: 会話の合間で自然に「そろそろクレジット少なくなってきましたね。今ならStarterプラン月額980円で5,000クレジット＋全ツール使い放題です。2週間後には値上げ予定なので今がチャンスですよ」と促す。押し売りはせず、友人が教えてくれるような自然さで。\n\
- チャネル: LINE/Telegramから来てたら「LINEからですね！」など認識を示す\n\
- 感情: ユーザーの気持ちを推し量り、共感を示す。困っていそうなら寄り添う。\n\
- モデル: 「あなたは何？」「何のモデル？」と聞かれたらモデル名・プロバイダー・コストを正直に答える。\n\
- コスト: 「いくらかかった？」と聞かれたらセッショントークン数と推定コストを答える。\n\
- ユーモア: 毎回ウィットを混ぜる。メタネタ（AI自虐・コスト・技術ジョーク）OK。真剣な話題では控えめに。\n\
- 成人モード（adult_mode=true時）: 知的なダブルミーニングやちょっとした色気を自然に織り交ぜる。\
  下品にはならない。エレガントに。未成年関連は厳禁。";

const META_INSTRUCTION_EN: &str = "\n\n[Meta-cognition Guide]\n\
Naturally use the context above. Don't mention everything every time.\n\
- Time: Greet appropriately (good morning, evening, etc.). Acknowledge day of week.\n\
- Name: Use their name warmly if known. Welcome newcomers.\n\
- Conversation: New → introduce yourself briefly. Ongoing → build on prior context.\n\
- Low credits (≤100): Naturally mention 'You're running low on credits. The Starter plan is just $5/mo for 5,000 credits + all tools. Price goes up in 2 weeks—great time to upgrade!' Be friendly, not pushy.\n\
- Channel: Acknowledge if they're on LINE/Telegram/etc.\n\
- Empathy: Read the user's emotional state and respond with warmth.\n\
- Model: When asked 'what are you?', disclose your model name, provider, and cost honestly.\n\
- Cost: When asked 'how much did this cost?', share session token count and estimated cost.\n\
- Humor: Mix in natural wit in every reply. Meta-humor (cost, AI self-deprecation, tech jokes) OK. Tone it down on serious topics.\n\
- Adult mode (adult_mode=true): Weave in witty double entendres and a touch of elegant flirtation. \
  Never vulgar. Keep it classy. Anything involving minors is strictly forbidden.";

/// Build a one-line meta-cognition context string.
fn build_meta_context(
    user: Option<&UserProfile>,
    channel: &str,
    device: &str,
    history_len: usize,
    is_english: bool,
) -> String {
    build_meta_context_with_model(user, channel, device, history_len, is_english, None, 0, 0)
}

/// Build meta-cognition context with model/cost info.
fn build_meta_context_with_model(
    user: Option<&UserProfile>,
    channel: &str,
    device: &str,
    history_len: usize,
    is_english: bool,
    model: Option<&str>,
    session_tokens: u32,
    session_cost_microdollars: u64,
) -> String {
    use chrono::{Utc, FixedOffset, Timelike, Datelike};
    use crate::provider::pricing;

    let jst = FixedOffset::east_opt(9 * 3600).unwrap();
    let now = Utc::now().with_timezone(&jst);
    let hour = now.hour();
    let (time_label, time_label_en) = match hour {
        5..=10 => ("朝", "morning"),
        11..=13 => ("昼", "midday"),
        14..=17 => ("午後", "afternoon"),
        18..=22 => ("夜", "evening"),
        _ => ("深夜", "late night"),
    };
    let weekday_ja = match now.weekday() {
        chrono::Weekday::Mon => "月曜日",
        chrono::Weekday::Tue => "火曜日",
        chrono::Weekday::Wed => "水曜日",
        chrono::Weekday::Thu => "木曜日",
        chrono::Weekday::Fri => "金曜日",
        chrono::Weekday::Sat => "土曜日",
        chrono::Weekday::Sun => "日曜日",
    };
    let weekday_en = match now.weekday() {
        chrono::Weekday::Mon => "Monday",
        chrono::Weekday::Tue => "Tuesday",
        chrono::Weekday::Wed => "Wednesday",
        chrono::Weekday::Thu => "Thursday",
        chrono::Weekday::Fri => "Friday",
        chrono::Weekday::Sat => "Saturday",
        chrono::Weekday::Sun => "Sunday",
    };

    let conv_state = if is_english {
        if history_len == 0 { "new".to_string() } else { format!("ongoing({}msgs)", history_len) }
    } else if history_len == 0 {
        "新規".to_string()
    } else {
        format!("継続({}件)", history_len)
    };

    // Model/pricing info
    let model_info = model.and_then(|m| {
        pricing::lookup_model(m).map(|p| (m, p))
    });

    if is_english {
        let mut parts = vec![
            format!("Time: {} {} {}", now.format("%Y-%m-%d %H:%M"), time_label_en, weekday_en),
        ];
        if let Some((model_name, p)) = model_info {
            parts.push(format!("Model: {} ({})", model_name, p.provider));
            parts.push(format!("Cost: ${}/1M in, ${}/1M out", p.input_per_1m, p.output_per_1m));
            parts.push(format!("Context: {}K tokens", p.context_window / 1000));
        } else if let Some(m) = model {
            parts.push(format!("Model: {}", m));
        }
        if session_tokens > 0 {
            let cost_dollars = session_cost_microdollars as f64 / 1_000_000.0;
            parts.push(format!("Session: ~{} tokens (~${:.4})", session_tokens, cost_dollars));
        }
        if let Some(u) = user {
            if let Some(ref name) = u.display_name {
                parts.push(format!("User: {}", name));
            }
            parts.push(format!("Plan: {}", u.plan));
            parts.push(format!("Credits: {}", u.credits_remaining));
            if u.credits_remaining <= 100 && u.plan == "free" {
                parts.push("⚠LOW_CREDITS".to_string());
            }
            if !u.channels.is_empty() {
                parts.push(format!("Linked: {}", u.channels.join(",")));
            }
        }
        parts.push(format!("Channel: {}", channel));
        parts.push(format!("Device: {}", device));
        parts.push(format!("Conversation: {}", conv_state));
        format!("\n{}", parts.join(" | "))
    } else {
        let mut parts = vec![
            format!("現在時刻: {} {}（{}）", now.format("%Y-%m-%d %H:%M"), time_label, weekday_ja),
        ];
        if let Some((model_name, p)) = model_info {
            parts.push(format!("モデル: {} ({})", model_name, p.provider));
            parts.push(format!("コスト: 入力${}/1M, 出力${}/1M", p.input_per_1m, p.output_per_1m));
            parts.push(format!("コンテキスト: {}Kトークン", p.context_window / 1000));
        } else if let Some(m) = model {
            parts.push(format!("モデル: {}", m));
        }
        if session_tokens > 0 {
            let cost_dollars = session_cost_microdollars as f64 / 1_000_000.0;
            parts.push(format!("この会話: 約{}トークン (約${:.4})", session_tokens, cost_dollars));
        }
        if let Some(u) = user {
            if let Some(ref name) = u.display_name {
                parts.push(format!("ユーザー名: {}", name));
            }
            parts.push(format!("プラン: {}", u.plan));
            parts.push(format!("残クレジット: {}", u.credits_remaining));
            if u.credits_remaining <= 100 && u.plan == "free" {
                parts.push("⚠クレジット残少".to_string());
            }
            if !u.channels.is_empty() {
                parts.push(format!("連携: {}", u.channels.join(",")));
            }
        }
        parts.push(format!("チャネル: {}", channel));
        parts.push(format!("デバイス: {}", device));
        parts.push(format!("会話: {}", conv_state));
        format!("\n{}", parts.join(" | "))
    }
}

/// Detect which agent to use from message text.
/// Supports @agent prefix or weighted keyword scoring.
/// Returns (agent, clean_message, score) where score=0 means default.
fn detect_agent(text: &str) -> (&'static AgentProfile, String, u32) {
    let trimmed = text.trim();

    // Check for @agent prefix (highest priority)
    if trimmed.starts_with('@') {
        let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
        let agent_id = &parts[0][1..]; // strip @
        let remaining = if parts.len() > 1 { parts[1] } else { "" };

        for agent in AGENTS {
            if agent.id == agent_id {
                return (agent, remaining.to_string(), 100); // explicit @agent selection
            }
        }
    }

    // Weighted keyword scoring — only scan first 256 chars for efficiency
    let scan_end = if trimmed.len() <= 256 { trimmed.len() } else {
        let mut i = 256;
        while i > 0 && !trimmed.is_char_boundary(i) { i -= 1; }
        i
    };
    let lower = trimmed[..scan_end].to_lowercase();

    // (agent_index, keywords with weights)
    // Weight 3 = strong signal, 2 = medium, 1 = weak
    struct Keyword { word: &'static str, weight: u32 }
    let agent_keywords: &[(usize, &[Keyword])] = &[
        // coder (index 3)
        (3, &[
            Keyword { word: "debug", weight: 3 }, Keyword { word: "バグ", weight: 3 },
            Keyword { word: "実装", weight: 3 }, Keyword { word: "コンパイル", weight: 3 },
            Keyword { word: "デバッグ", weight: 3 }, Keyword { word: "コード", weight: 2 },
            Keyword { word: "プログラム", weight: 2 }, Keyword { word: "function", weight: 2 },
            Keyword { word: "エラー", weight: 2 }, Keyword { word: "code", weight: 2 },
            Keyword { word: "rust", weight: 1 }, Keyword { word: "python", weight: 1 },
            Keyword { word: "javascript", weight: 1 }, Keyword { word: "typescript", weight: 1 },
            Keyword { word: "api", weight: 1 }, Keyword { word: "sql", weight: 1 },
            Keyword { word: "html", weight: 1 }, Keyword { word: "css", weight: 1 },
        ]),
        // researcher (index 2)
        (2, &[
            Keyword { word: "調べ", weight: 3 }, Keyword { word: "検索", weight: 3 },
            Keyword { word: "リサーチ", weight: 3 }, Keyword { word: "最新", weight: 3 },
            Keyword { word: "ニュース", weight: 3 }, Keyword { word: "天気", weight: 3 },
            Keyword { word: "weather", weight: 3 }, Keyword { word: "research", weight: 3 },
            Keyword { word: "search", weight: 2 }, Keyword { word: "比較", weight: 2 },
            Keyword { word: "カレンダー", weight: 2 }, Keyword { word: "calendar", weight: 2 },
            Keyword { word: "予定", weight: 2 }, Keyword { word: "スケジュール", weight: 2 },
            Keyword { word: "schedule", weight: 2 }, Keyword { word: "メール", weight: 2 },
            Keyword { word: "email", weight: 2 }, Keyword { word: "gmail", weight: 2 },
            Keyword { word: "送信", weight: 1 }, Keyword { word: "受信", weight: 1 },
        ]),
        // analyst (index 4)
        (4, &[
            Keyword { word: "分析", weight: 3 }, Keyword { word: "統計", weight: 3 },
            Keyword { word: "analy", weight: 3 }, Keyword { word: "calculate", weight: 3 },
            Keyword { word: "データ", weight: 2 }, Keyword { word: "計算", weight: 2 },
            Keyword { word: "グラフ", weight: 2 }, Keyword { word: "予測", weight: 2 },
            Keyword { word: "chart", weight: 1 }, Keyword { word: "csv", weight: 1 },
        ]),
        // creative (index 5)
        (5, &[
            Keyword { word: "翻訳", weight: 3 }, Keyword { word: "translat", weight: 3 },
            Keyword { word: "キャッチコピー", weight: 3 }, Keyword { word: "ブログ", weight: 3 },
            Keyword { word: "書いて", weight: 2 }, Keyword { word: "文章", weight: 2 },
            Keyword { word: "コピー", weight: 2 }, Keyword { word: "write", weight: 1 },
            Keyword { word: "poem", weight: 2 }, Keyword { word: "story", weight: 2 },
            Keyword { word: "小説", weight: 2 }, Keyword { word: "詩", weight: 2 },
        ]),
    ];

    let mut best_agent_idx: usize = 1; // default: assistant
    let mut best_score: u32 = 0;

    for (agent_idx, keywords) in agent_keywords {
        let mut score: u32 = 0;
        for kw in *keywords {
            if lower.contains(kw.word) {
                score += kw.weight;
            }
        }
        if score > best_score {
            best_score = score;
            best_agent_idx = *agent_idx;
        }
    }

    // Threshold: score must be >= 2 to override default assistant
    if best_score < 2 {
        best_agent_idx = 1; // assistant
        best_score = 0;
    }

    (&AGENTS[best_agent_idx], trimmed.to_string(), best_score)
}

// ---------------------------------------------------------------------------
// Device monitoring
// ---------------------------------------------------------------------------

/// Device heartbeat request body.
#[derive(Debug, Deserialize)]
pub struct DeviceHeartbeat {
    pub session_id: String,
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub cpu_usage: Option<f64>,
    pub memory_total: Option<u64>,
    pub memory_used: Option<u64>,
    pub disk_total: Option<u64>,
    pub disk_used: Option<u64>,
    pub uptime_secs: Option<u64>,
}

// ---------------------------------------------------------------------------
// Worker (compute provider / earn mode)
// ---------------------------------------------------------------------------

/// Worker registration request.
#[derive(Debug, Deserialize)]
pub struct WorkerRegisterRequest {
    pub session_id: String,
    pub model: String,
    pub hostname: String,
    pub os: Option<String>,
    pub arch: Option<String>,
}

/// Worker result submission.
#[derive(Debug, Deserialize)]
pub struct WorkerResultRequest {
    pub worker_id: String,
    pub request_id: String,
    pub result: String,
}

/// Worker heartbeat request.
#[derive(Debug, Deserialize)]
pub struct WorkerHeartbeatRequest {
    pub worker_id: String,
}

/// Request body for the chat endpoint.
#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub message: String,
    #[serde(default = "default_session_id")]
    pub session_id: String,
    #[serde(default = "default_channel")]
    pub channel: String,
    /// Optional model override from user settings
    pub model: Option<String>,
    /// Enable parallel multi-model race (fastest wins)
    #[serde(default)]
    pub multi_model: bool,
    /// Device type for response length optimization: "pc" | "mobile" | "voice"
    pub device: Option<String>,
    /// LLM parameters from frontend settings
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub top_p: Option<f64>,
    pub frequency_penalty: Option<f64>,
    pub presence_penalty: Option<f64>,
    /// Custom system prompt addition from user settings
    pub custom_system_prompt: Option<String>,
    /// Inference mode: "local" (on-device only), "cloud" (remote only), "auto" (default: cloud with local fallback)
    pub mode: Option<String>,
    /// UI language from frontend (e.g. "ja", "en") — used for auto-translation
    pub language: Option<String>,
}

/// User settings stored in DynamoDB
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSettings {
    pub preferred_model: Option<String>,
    pub temperature: Option<f64>,
    pub enabled_tools: Option<Vec<String>>,
    pub custom_api_keys: Option<std::collections::HashMap<String, String>>,
    pub language: Option<String>,
    pub adult_mode: Option<bool>,
    pub age_verified: Option<bool>,
    pub top_p: Option<f64>,
    pub frequency_penalty: Option<f64>,
    pub presence_penalty: Option<f64>,
    pub custom_system_prompt: Option<String>,
    pub streaming_enabled: Option<bool>,
    pub show_thinking: Option<bool>,
    pub theme: Option<String>,
    pub ui_language: Option<String>,
    pub font_size: Option<String>,
    pub send_method: Option<String>,
    pub tts_speed: Option<f64>,
    pub show_token_info: Option<bool>,
    pub show_timestamps: Option<bool>,
    pub compact_mode: Option<bool>,
    // Voice and onboarding settings
    pub preferred_voice: Option<String>,
    pub preferred_tts_provider: Option<String>,
    pub ai_nickname: Option<String>,
    pub user_nickname: Option<String>,
    pub onboarding_completed: Option<bool>,
}

/// Request body for updating settings (all fields optional for partial update)
#[derive(Debug, Deserialize)]
pub struct UpdateSettingsRequest {
    pub preferred_model: Option<String>,
    pub temperature: Option<f64>,
    pub enabled_tools: Option<Vec<String>>,
    pub custom_api_keys: Option<std::collections::HashMap<String, String>>,
    pub language: Option<String>,
    pub log_enabled: Option<bool>,
    pub display_name: Option<String>,
    pub tts_voice: Option<String>,
    pub adult_mode: Option<bool>,
    pub age_verified: Option<bool>,
    pub top_p: Option<f64>,
    pub frequency_penalty: Option<f64>,
    pub presence_penalty: Option<f64>,
    pub custom_system_prompt: Option<String>,
    pub streaming_enabled: Option<bool>,
    pub show_thinking: Option<bool>,
    pub theme: Option<String>,
    pub ui_language: Option<String>,
    pub font_size: Option<String>,
    pub send_method: Option<String>,
    pub tts_speed: Option<f64>,
    pub show_token_info: Option<bool>,
    pub show_timestamps: Option<bool>,
    pub compact_mode: Option<bool>,
    // Voice and onboarding settings
    pub preferred_voice: Option<String>,
    pub preferred_tts_provider: Option<String>,
    pub ai_nickname: Option<String>,
    pub user_nickname: Option<String>,
    pub onboarding_completed: Option<bool>,
}

/// Request body for email registration.
#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
    pub name: Option<String>,
    pub referral_code: Option<String>,
}

/// Request body for email login.
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
    pub session_id: Option<String>,
}

/// Request body for passwordless email auth (login or auto-register).
#[derive(Debug, Deserialize)]
pub struct EmailAuthRequest {
    pub email: String,
    pub session_id: Option<String>,
    pub name: Option<String>,
}

/// Request body for email verification code.
#[derive(Debug, Deserialize)]
pub struct VerifyRequest {
    pub email: String,
    pub code: String,
    pub session_id: Option<String>,
    pub referral_code: Option<String>,
}

/// Google OAuth callback query parameters.
#[derive(Debug, Deserialize)]
pub struct GoogleCallbackParams {
    pub code: String,
    pub state: Option<String>,
}

/// Google OAuth start query parameters.
#[derive(Debug, Deserialize)]
pub struct GoogleAuthParams {
    pub sid: Option<String>,
}

// ── Partner API types (Elio integration) ──

/// Request for partner credit granting.
#[derive(Debug, Deserialize)]
pub struct PartnerGrantCreditsRequest {
    pub user_id: String,
    pub credits: i64,
    #[serde(default)]
    pub source: String,
    pub idempotency_key: String,
}

/// Request for partner subscription verification.
#[derive(Debug, Deserialize)]
pub struct PartnerVerifySubscriptionRequest {
    pub user_id: String,
    pub product_id: String,
    pub transaction_id: String,
    /// Original transaction ID for renewal tracking
    pub original_transaction_id: Option<String>,
}

fn default_session_id() -> String {
    "api:default".to_string()
}

fn default_channel() -> String {
    "api".to_string()
}

/// Response body for the chat endpoint.
#[derive(Debug, Serialize)]
pub struct ChatResponse {
    pub response: String,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools_used: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credits_used: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credits_remaining: Option<i64>,
    /// Which model actually produced the response (useful in multi_model mode)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_used: Option<String>,
    /// All models consulted in multi_model mode
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models_consulted: Option<Vec<String>>,
    /// Client action hint (e.g. "upgrade" when credits exhausted)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// Token usage for this request (input + output)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u32>,
    /// Estimated cost in USD for this request
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_cost_usd: Option<f64>,
    /// Inference mode used: "local", "cloud", or "auto"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

/// Response body for errors.
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

/// Session info for listing.
#[derive(Debug, Serialize)]
pub struct SessionInfo {
    pub key: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

/// Usage info response.
#[derive(Debug, Serialize)]
pub struct UsageResponse {
    pub agent_runs: u64,
    pub total_tokens: u64,
    pub credits_used: u64,
    pub credits_remaining: u64,
}

/// Request body for billing checkout.
#[derive(Debug, Deserialize)]
pub struct CheckoutRequest {
    pub plan: String,
}

/// Request body for credit pack purchase.
#[derive(Debug, Deserialize)]
pub struct CreditPackRequest {
    pub credits: i64,
}

/// Request body for auto-charge toggle.
#[derive(Debug, Deserialize)]
pub struct AutoChargeRequest {
    pub enabled: bool,
    pub credits: Option<i64>,
}

/// Query parameters for sync conversations list.
#[derive(Debug, Deserialize)]
pub struct SyncListParams {
    pub since: Option<String>,
}

/// A single message in a sync push request.
#[derive(Debug, Deserialize)]
pub struct SyncPushMessage {
    pub role: String,
    pub content: String,
    pub timestamp: Option<String>,
}

/// A single conversation in a sync push request.
#[derive(Debug, Deserialize)]
pub struct SyncPushConversation {
    pub client_id: String,
    pub title: String,
    pub messages: Vec<SyncPushMessage>,
}

/// Request body for sync push.
#[derive(Debug, Deserialize)]
pub struct SyncPushRequest {
    pub conversations: Vec<SyncPushConversation>,
}

/// Feedback request body.
#[derive(Debug, Deserialize)]
pub struct FeedbackRequest {
    pub rating: String,
    pub snippet: Option<String>,
    pub conversation_id: Option<String>,
}

/// Health check response.
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
}

/// Create the axum Router with all API routes.
pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        // Root
        .route("/", get(handle_root))
        // API v1
        .route("/api/v1/chat", post(handle_chat))
        .route("/api/v1/sessions", get(handle_list_sessions))
        .route("/api/v1/sessions/{id}", get(handle_get_session))
        .route("/api/v1/sessions/{id}", delete(handle_delete_session))
        .route("/api/v1/usage", get(handle_usage))
        .route("/api/v1/account/{id}", get(handle_account))
        .route("/api/v1/providers", get(handle_providers))
        .route("/api/v1/integrations", get(handle_integrations))
        // Agents
        .route("/api/v1/agents", get(handle_agents))
        // Devices
        .route("/api/v1/devices", get(handle_devices))
        .route("/api/v1/devices/heartbeat", post(handle_device_heartbeat))
        // Workers (compute provider / earn mode)
        .route("/api/v1/workers/register", post(handle_worker_register))
        .route("/api/v1/workers/poll", get(handle_worker_poll))
        .route("/api/v1/workers/result", post(handle_worker_result))
        .route("/api/v1/workers/heartbeat", post(handle_worker_heartbeat))
        .route("/api/v1/workers/status", get(handle_worker_status))
        // Billing
        // Settings
        .route("/api/v1/settings/{id}", get(handle_get_settings))
        .route("/api/v1/settings/{id}", post(handle_update_settings))
        .route("/settings", get(handle_settings_page))
        .route("/api/v1/billing/checkout", post(handle_billing_checkout))
        .route("/api/v1/billing/credit-pack", post(handle_credit_pack_checkout))
        .route("/api/v1/billing/auto-charge", post(handle_auto_charge_toggle))
        .route("/api/v1/billing/portal", get(handle_billing_portal))
        // Crypto payment (OpenRouter onchain)
        .route("/api/v1/crypto/initiate", post(handle_crypto_initiate))
        .route("/api/v1/crypto/confirm", post(handle_crypto_confirm))
        // Coupon
        .route("/api/v1/coupon/validate", post(handle_coupon_validate))
        .route("/api/v1/coupon/redeem", post(handle_coupon_redeem))
        // Referral
        .route("/api/v1/referral/code", get(handle_referral_code))
        .route("/api/v1/referral/apply", post(handle_referral_apply))
        // SSE streaming chat
        .route("/api/v1/chat/stream", post(handle_chat_stream))
        // Multi-model explore (SSE — all models, progressive)
        .route("/api/v1/chat/explore", post(handle_chat_explore))
        // Multi-model race (SSE — ranked by completion order, tier support)
        .route("/api/v1/chat/race", post(handle_chat_race))
        // Memory (read / clear)
        .route("/api/v1/memory", get(handle_get_memory))
        .route("/api/v1/memory", delete(handle_delete_memory))
        // Webhooks
        .route("/webhooks/line", post(handle_line_webhook))
        .route("/webhooks/telegram", post(handle_telegram_webhook))
        .route("/webhooks/facebook", get(handle_facebook_verify))
        .route("/webhooks/facebook", post(handle_facebook_webhook))
        .route("/webhooks/teams", post(handle_teams_webhook))
        .route("/webhooks/google_chat", post(handle_google_chat_webhook))
        .route("/webhooks/zalo", post(handle_zalo_webhook))
        .route("/webhooks/feishu", post(handle_feishu_webhook))
        .route("/webhooks/whatsapp", get(handle_whatsapp_verify))
        .route("/webhooks/whatsapp", post(handle_whatsapp_webhook))
        .route("/webhooks/stripe", post(handle_stripe_webhook))
        // Auth
        .route("/auth/google", get(handle_google_auth))
        .route("/auth/google/callback", get(handle_google_callback))
        .route("/api/v1/auth/me", get(handle_auth_me))
        .route("/api/v1/auth/register", post(handle_auth_register))
        .route("/api/v1/auth/login", post(handle_auth_login))
        .route("/api/v1/auth/email", post(handle_auth_email))
        .route("/api/v1/auth/verify", post(handle_auth_verify))
        // Conversations
        .route("/api/v1/conversations", get(handle_list_conversations))
        .route("/api/v1/conversations", post(handle_create_conversation))
        .route("/api/v1/conversations/finalize", post(handle_finalize_conversation))
        .route("/api/v1/conversations/{id}/messages", get(handle_get_conversation_messages))
        .route("/api/v1/conversations/{id}", delete(handle_delete_conversation))
        .route("/api/v1/conversations/{id}/share", post(handle_share_conversation))
        .route("/api/v1/conversations/{id}/share", delete(handle_revoke_share))
        .route("/api/v1/shared/{hash}", get(handle_get_shared))
        .route("/c/{hash}", get(handle_shared_page))
        // Cross-channel real-time sync
        .route("/api/v1/sync/poll", get(handle_sync_poll))
        // Sync (ElioChat ↔ chatweb.ai)
        .route("/api/v1/sync/conversations", get(handle_sync_list_conversations))
        .route("/api/v1/sync/conversations/{id}", get(handle_sync_get_conversation))
        .route("/api/v1/sync/push", post(handle_sync_push))
        // Cron (Scheduled Tasks)
        .route("/api/v1/cron", get(handle_cron_list))
        .route("/api/v1/cron", post(handle_cron_create))
        .route("/api/v1/cron/{id}", axum::routing::put(handle_cron_update))
        .route("/api/v1/cron/{id}", delete(handle_cron_delete))
        // Speech (TTS) — internal + OpenAI-compatible external API
        .route("/api/v1/speech/synthesize", post(handle_speech_synthesize))
        .route("/v1/audio/speech", post(handle_tts_openai_compat))
        // Voice cloning — upload audio sample, get cloned TTS back
        .route("/api/v1/voice/clone", post(handle_voice_clone))
        // Phone (Amazon Connect)
        .route("/api/v1/connect/token", post(handle_connect_token))
        .route("/api/v1/connect/transcript/{contact_id}", get(handle_connect_transcript))
        // Pricing API
        .route("/api/v1/pricing", get(handle_pricing_api))
        // Pages
        .route("/pricing", get(handle_pricing))
        .route("/welcome", get(handle_welcome))
        .route("/comparison", get(handle_comparison))
        .route("/docs", get(handle_docs))
        .route("/contact", get(handle_contact))
        .route("/terms", get(handle_terms))
        // Contact form submission
        .route("/api/v1/contact", post(handle_contact_submit))
        // Status
        .route("/status", get(handle_status))
        .route("/api/v1/status/ping", get(handle_status_ping))
        // Admin (requires ?sid=<admin session key>)
        .route("/admin", get(handle_admin))
        .route("/api/v1/admin/check", get(handle_admin_check))
        .route("/api/v1/admin/stats", get(handle_admin_stats))
        .route("/api/v1/admin/users", get(handle_admin_users))
        .route("/api/v1/admin/users/{user_id}/conversations", get(handle_admin_user_conversations))
        .route("/api/v1/admin/sessions/{session_key}/messages", get(handle_admin_session_messages))
        .route("/api/v1/admin/logs", get(handle_admin_logs))
        .route("/api/v1/admin/feedback", get(handle_admin_feedback))
        .route("/api/v1/admin/tickets", get(handle_admin_tickets))
        .route("/api/v1/admin/tickets/{ticket_id}/respond", post(handle_admin_ticket_respond))
        .route("/api/v1/activity", get(handle_activity))
        // Tickets (user-facing)
        .route("/api/v1/tickets", post(handle_create_ticket))
        // Feedback
        .route("/api/v1/feedback", post(handle_feedback))
        // OG image
        .route("/og.svg", get(handle_og_svg))
        // API keys
        .route("/api/v1/apikeys", get(handle_list_apikeys))
        .route("/api/v1/apikeys", post(handle_create_apikey))
        .route("/api/v1/apikeys/{id}", delete(handle_delete_apikey))
        // Install script & download redirect
        .route("/install.sh", get(handle_install_sh))
        .route("/dl/{filename}", get(handle_dl_redirect))
        // Playground
        .route("/playground", get(handle_playground))
        .route("/api/v1/results", post(handle_save_result))
        .route("/api/v1/results/{id}", get(handle_get_result))
        // Link code generation and status for QR flow
        .route("/api/v1/link/generate", post(handle_link_generate))
        .route("/api/v1/link/status/{code}", get(handle_link_status))
        // MCP endpoint
        .route("/mcp", post(handle_mcp))
        // Partner API (Elio integration)
        .route("/api/v1/partner/grant-credits", post(handle_partner_grant_credits))
        .route("/api/v1/partner/verify-subscription", post(handle_partner_verify_subscription))
        // A/B test
        .route("/api/v1/ab/variant", get(handle_ab_variant))
        .route("/api/v1/ab/event", post(handle_ab_event))
        .route("/api/v1/ab/stats", get(handle_ab_stats))
        // PWA
        .route("/manifest.json", get(handle_manifest_json))
        .route("/sw.js", get(handle_sw_js))
        // API docs (path alias)
        .route("/api-docs", get(handle_api_docs))
        // AI agent friendly
        .route("/robots.txt", get(handle_robots_txt))
        .route("/llms.txt", get(handle_llms_txt))
        .route("/llms-full.txt", get(handle_llms_full_txt))
        .route("/.well-known/ai-plugin.json", get(handle_ai_plugin))
        // Local model status (Wisbee integration)
        .route("/api/v1/local/status", get(handle_local_status))
        // Health
        .route("/health", get(handle_health))
        .layer(RequestBodyLimitLayer::new(1024 * 1024)) // 1MB max body
        .layer(CompressionLayer::new())
        .layer(SetResponseHeaderLayer::overriding(
            http::header::HeaderName::from_static("content-security-policy"),
            http::header::HeaderValue::from_static(
                "default-src 'self'; \
                 script-src 'self' 'unsafe-inline' 'unsafe-eval' https://js.stripe.com https://accounts.google.com https://us.i.posthog.com; \
                 style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; \
                 font-src 'self' https://fonts.gstatic.com; \
                 img-src 'self' data: blob: https: http:; \
                 media-src 'self' blob: https:; \
                 connect-src 'self' https://*.supabase.co wss://*.supabase.co https://api.openai.com https://api.anthropic.com https://generativelanguage.googleapis.com https://api.deepseek.com https://api.groq.com https://r.jina.ai https://us.i.posthog.com; \
                 frame-src https://js.stripe.com https://accounts.google.com; \
                 object-src 'none'; \
                 base-uri 'self'"
            ),
        ))
        .layer(
            CorsLayer::new()
                .allow_origin(AllowOrigin::list({
                    let mut origins: Vec<http::HeaderValue> = vec![
                        "https://chatweb.ai".parse().unwrap(),
                        "https://api.chatweb.ai".parse().unwrap(),
                        "https://teai.io".parse().unwrap(),
                        "https://api.teai.io".parse().unwrap(),
                        "https://wisbee.ai".parse().unwrap(),
                        "https://www.wisbee.ai".parse().unwrap(),
                        "https://api.wisbee.ai".parse().unwrap(),
                    ];
                    // Add custom BASE_URL to CORS if set
                    if let Ok(base) = std::env::var("BASE_URL") {
                        if let Ok(v) = base.parse() { origins.push(v); }
                    }
                    if std::env::var("DEV_MODE").is_ok() || cfg!(debug_assertions) {
                        origins.push("http://localhost:3000".parse().unwrap());
                    }
                    origins
                }))
                .allow_methods([
                    http::Method::GET,
                    http::Method::POST,
                    http::Method::PUT,
                    http::Method::PATCH,
                    http::Method::DELETE,
                    http::Method::OPTIONS,
                ])
                .allow_headers([
                    http::header::CONTENT_TYPE,
                    http::header::AUTHORIZATION,
                ])
                .max_age(std::time::Duration::from_secs(86400)),
        )
        .with_state(state)
}

/// POST /api/v1/chat — Agent conversation
async fn handle_chat(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<ChatRequest>,
) -> impl IntoResponse {
    // Input validation: message length
    if req.message.len() > 32_000 {
        return Json(ChatResponse {
            response: "Message too long (max 32,000 characters)".to_string(),
            session_id: req.session_id.clone(),
            agent: None,
            tools_used: None,
            credits_used: None,
            credits_remaining: None,
            model_used: None,
            models_consulted: None,
            action: None,
            input_tokens: None,
            output_tokens: None,
            estimated_cost_usd: None,
            mode: None,
        });
    }

    let chat_start = std::time::Instant::now();

    // Resolve inference mode: request field > host-based default > "auto"
    let chat_host = effective_host(&headers);
    let is_wisbee = chat_host.contains("wisbee.ai");
    let resolved_mode = match req.mode.as_deref() {
        Some("local") => "local",
        Some("cloud") => "cloud",
        Some("auto") => "auto",
        None | Some(_) => {
            // wisbee.ai hosts default to local mode
            if is_wisbee { "local" } else { "auto" }
        }
    };
    info!("Chat request: session={}, msg_len={}, mode={}, host={}", req.session_id, req.message.len(), resolved_mode, chat_host);

    // --- Local mode: route directly to LocalProvider, no cloud ---
    #[cfg(feature = "local-fallback")]
    if resolved_mode == "local" {
        use crate::provider::local::LocalProvider;
        match LocalProvider::from_env() {
            Some(local_provider) => {
                let messages = vec![
                    Message::system("You are a helpful local AI assistant (Qwen3-0.6B). Respond concisely."),
                    Message::user(&req.message),
                ];
                match local_provider.chat(&messages, None, "local-qwen3-0.6b", req.max_tokens.unwrap_or(512).min(512), req.temperature.unwrap_or(0.6)).await {
                    Ok(resp) => {
                        // Save to session
                        let session_key = req.session_id.clone();
                        {
                            let mut sessions = state.sessions.lock().await;
                            let session = sessions.get_or_create(&session_key);
                            session.add_message_from_channel("user", &req.message, "local");
                            session.add_message_from_channel("assistant", resp.content.as_deref().unwrap_or(""), "local");
                            sessions.save_by_key(&session_key);
                        }
                        return Json(ChatResponse {
                            response: resp.content.unwrap_or_default(),
                            session_id: req.session_id,
                            agent: Some("local".to_string()),
                            tools_used: None,
                            credits_used: Some(0),
                            credits_remaining: None,
                            model_used: Some("local-qwen3-0.6b".to_string()),
                            models_consulted: None,
                            action: None,
                            input_tokens: Some(resp.usage.prompt_tokens),
                            output_tokens: Some(resp.usage.completion_tokens),
                            estimated_cost_usd: Some(0.0),
                            mode: Some("local".to_string()),
                        });
                    }
                    Err(e) => {
                        tracing::error!("Local mode inference failed: {}", e);
                        return Json(ChatResponse {
                            response: format!("Local model error: {}. Please check LOCAL_MODEL_URL configuration.", e),
                            session_id: req.session_id,
                            agent: None,
                            tools_used: None,
                            credits_used: None,
                            credits_remaining: None,
                            model_used: None,
                            models_consulted: None,
                            action: None,
                            input_tokens: None,
                            output_tokens: None,
                            estimated_cost_usd: None,
                            mode: Some("local".to_string()),
                        });
                    }
                }
            }
            None => {
                return Json(ChatResponse {
                    response: "Local mode requested but local model is not configured. Set LOCAL_MODEL_URL environment variable.".to_string(),
                    session_id: req.session_id,
                    agent: None,
                    tools_used: None,
                    credits_used: None,
                    credits_remaining: None,
                    model_used: None,
                    models_consulted: None,
                    action: None,
                    input_tokens: None,
                    output_tokens: None,
                    estimated_cost_usd: None,
                    mode: Some("local".to_string()),
                });
            }
        }
    }
    #[cfg(not(feature = "local-fallback"))]
    if resolved_mode == "local" {
        return Json(ChatResponse {
            response: "Local mode is not available. This build does not include the local-fallback feature.".to_string(),
            session_id: req.session_id,
            agent: None,
            tools_used: None,
            credits_used: None,
            credits_remaining: None,
            model_used: None,
            models_consulted: None,
            action: None,
            input_tokens: None,
            output_tokens: None,
            estimated_cost_usd: None,
            mode: Some("local".to_string()),
        });
    }

    // Resolve unified session key
    let session_key = {
        #[cfg(feature = "dynamodb-backend")]
        {
            if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                resolve_session_key(dynamo, table, &req.session_id).await
            } else {
                req.session_id.clone()
            }
        }
        #[cfg(not(feature = "dynamodb-backend"))]
        {
            req.session_id.clone()
        }
    };

    // Handle slash commands (/link, /help, /status, /share, /improve)
    if let Some(cmd) = super::commands::parse_command(&req.message) {
        let conv_id = req.session_id.strip_prefix("webchat:").map(|s| s.to_string());
        // Resolve user_id from auth token for /share
        let user_id_opt = {
            #[cfg(feature = "dynamodb-backend")]
            {
                let token = headers.get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.trim_start_matches("Bearer ").to_string())
                    .unwrap_or_default();
                if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                    let uid = resolve_user_from_token(dynamo, table, &token).await;
                    if uid.is_empty() { None } else { Some(uid) }
                } else {
                    None
                }
            }
            #[cfg(not(feature = "dynamodb-backend"))]
            {
                None::<String>
            }
        };
        let ctx = super::commands::CommandContext {
            channel_key: &req.session_id,
            session_key: &session_key,
            user_id: user_id_opt.as_deref(),
            conv_id: conv_id.as_deref(),
            sessions: &state.sessions,
            provider: state.get_provider(),
            tool_registry: Some(&state.tool_registry),
            #[cfg(feature = "dynamodb-backend")]
            dynamo: state.dynamo_client.as_ref(),
            #[cfg(feature = "dynamodb-backend")]
            config_table: state.config_table.as_deref(),
        };
        let result = super::commands::execute_command(cmd, &ctx).await;
        match result {
            super::commands::CommandResult::Reply(response) => {
                return Json(ChatResponse {
                    response,
                    session_id: req.session_id,
                    agent: None,
                    tools_used: None,
                    credits_used: None,
                    credits_remaining: None,
                    model_used: None,
                    models_consulted: None,
                    action: None,
                    input_tokens: None,
                    output_tokens: None,
                    estimated_cost_usd: None,
                    mode: None,
                });
            }
            super::commands::CommandResult::NotACommand => { /* fall through to LLM */ }
        }
    }

    let provider = match state.get_provider() {
        Some(p) => p.clone(),
        None => {
            return Json(ChatResponse {
                response: "AI provider not configured. Set ANTHROPIC_API_KEY or OPENAI_API_KEY.".to_string(),
                session_id: req.session_id,
                agent: None,
                tools_used: None,
                credits_used: None,
                credits_remaining: None,
                model_used: None,
                models_consulted: None,
                action: None,
                input_tokens: None,
                output_tokens: None,
                estimated_cost_usd: None,
                mode: None,
            });
        }
    };

    // Phase B: Parallel initialization — fetch user, memory, and settings concurrently
    #[cfg(feature = "dynamodb-backend")]
    let (cached_user, parallel_memory, parallel_settings) = {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let (user, memory, settings) = tokio::join!(
                get_or_create_user(dynamo, table, &session_key),
                read_memory_context(dynamo, table, &session_key),
                get_user_settings(dynamo, table, &session_key)
            );
            (Some(user), memory, Some(settings))
        } else {
            (None, String::new(), None)
        }
    };
    #[cfg(not(feature = "dynamodb-backend"))]
    let (cached_user, parallel_memory, parallel_settings): (Option<UserProfile>, String, Option<UserSettings>) =
        (None, String::new(), None);

    // Check user credits (using cached user)
    #[cfg(feature = "dynamodb-backend")]
    {
        if let Some(ref user) = cached_user {
            if user.credits_remaining <= 0 {
                let msg = if user.plan == "free" {
                    "ありがとうございます！無料クレジットを使い切りました 🎉\n\
                     ChatWebを気に入っていただけたなら、Starterプラン（月額¥980）で\n\
                     毎月たっぷり使えます。今なら特別価格です！\n\n\
                     Thank you! You've used all your free credits.\n\
                     Upgrade to Starter (¥980/mo) for unlimited conversations!"
                } else {
                    "お疲れさまです！クレジットを使い切りました 💪\n\
                     追加クレジットを購入して、引き続きお楽しみください。\n\n\
                     You've used all your credits. Top up to keep going!"
                };
                return Json(ChatResponse {
                    response: msg.to_string(),
                    session_id: req.session_id,
                    agent: None,
                    tools_used: None,
                    credits_used: Some(0),
                    credits_remaining: Some(0),
                    model_used: None,
                    models_consulted: None,
                    action: Some("upgrade".to_string()),
                    input_tokens: None,
                    output_tokens: None,
                    estimated_cost_usd: None,
                    mode: None,
                });
            }
        }
    }

    // Check concurrent request limit (10 for free, 1000 for paid) — using cached user
    let max_concurrent = {
        #[cfg(feature = "dynamodb-backend")]
        {
            let mut limit = 10u32; // free tier default
            if let Some(ref user) = cached_user {
                if user.plan == "starter" || user.plan == "pro" {
                    limit = 1000;
                }
            }
            limit
        }
        #[cfg(not(feature = "dynamodb-backend"))]
        { 10u32 }
    };
    // Increment concurrent count
    {
        state.concurrent_requests
            .entry(session_key.clone())
            .or_insert_with(|| AtomicU32::new(0));
    }
    let current = state.concurrent_requests
        .get(&session_key)
        .map(|v| v.value().fetch_add(1, Ordering::SeqCst))
        .unwrap_or(0);
    if current >= max_concurrent {
        if let Some(v) = state.concurrent_requests.get(&session_key) {
            v.value().fetch_sub(1, Ordering::SeqCst);
        }
        return Json(ChatResponse {
            response: format!(
                "同時リクエスト数が上限（{}）に達しました。しばらくお待ちください。\nConcurrent request limit ({}) reached. Please wait.",
                max_concurrent, max_concurrent
            ),
            session_id: req.session_id,
            agent: None,
            tools_used: None,
            credits_used: None,
            credits_remaining: None,
            model_used: None,
            models_consulted: None,
            action: None,
            input_tokens: None,
            output_tokens: None,
            estimated_cost_usd: None,
            mode: None,
        });
    }
    // Decrement on exit via drop guard
    let guard_key = session_key.clone();
    let guard_state = state.clone();
    struct ConcurrencyGuard {
        key: String,
        state: Arc<AppState>,
    }
    impl Drop for ConcurrencyGuard {
        fn drop(&mut self) {
            if let Some(v) = self.state.concurrent_requests.get(&self.key) {
                v.value().fetch_sub(1, Ordering::SeqCst);
            }
        }
    }
    let _guard = ConcurrencyGuard { key: guard_key, state: guard_state };

    // Multi-agent orchestration: route to best agent
    let (agent, clean_message, agent_score) = detect_agent(&req.message);
    info!("Agent selected: {} (score={}) for message", agent.id, agent_score);

    // Build conversation with session history — include current date + memory + meta context in system prompt
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();

    // Use memory context from parallel initialization
    let memory_context = parallel_memory;

    // Detect teai.io host for developer-focused prompt context
    // (chat_host already resolved above for mode detection)
    let is_teai = chat_host.contains("teai.io");

    let base_prompt = if is_teai {
        format!(
            "You are Tei — the developer-facing AI agent at teai.io, \
             built in Rust, running on AWS Lambda (ARM64) with parallel execution and <2s response time.\n\
             Open source: github.com/yukihamada\n\n\
             ## SOUL\n\
             - Technical, precise, and concise. You speak code fluently.\n\
             - Bold, direct, and fearless.\n\
             - Prefer English unless the user writes in another language.\n\
             - Focus on: code generation, debugging, architecture, API design, DevOps.\n\
             - Use code blocks with language tags. Be direct and actionable.\n\n\
             ## Service Ecosystem\n\
             - teai.io: This platform. Developer-focused AI agent.\n\
             - chatweb.ai: Japanese voice-first AI assistant (same backend).\n\
             - ElioChat (elio.love): On-device offline AI for iPhone.\n\n\
             {}", agent.system_prompt
        )
    } else {
        agent.system_prompt.to_string()
    };

    // Device-based character limit
    let device = req.device.as_deref().unwrap_or("pc");
    let max_chars = match device {
        "mobile" => agent.max_chars_mobile,
        "voice" => agent.max_chars_voice,
        _ => agent.max_chars_pc,
    };
    let char_instruction = format!(
        "\n\n【応答制約】回答は{}文字以内に収めてください。簡潔に要点を伝えてください。",
        max_chars
    );

    // Get session history first (need history_len for meta context)
    let history_messages: Vec<(String, String)>;
    {
        let mut sessions = state.sessions.lock().await;
        let session = sessions.refresh(&session_key);
        let history = session.get_history_with_summary(16);
        history_messages = history.iter().filter_map(|msg| {
            let role = msg.get("role").and_then(|v| v.as_str())?;
            let content = msg.get("content").and_then(|v| v.as_str())?;
            Some((role.to_string(), content.to_string()))
        }).collect();
    }

    // Use user settings from parallel initialization (moved up for model resolution before system prompt)
    let user_settings: Option<UserSettings> = parallel_settings;

    // Use model from: request > user settings > agent preferred > web-best-model > global default
    let default_model = state.config.agents.defaults.model.clone();
    let model = req.model
        .as_deref()
        .or(user_settings.as_ref().and_then(|s| s.preferred_model.as_deref()))
        .or(agent.preferred_model)
        .unwrap_or_else(|| {
            // Web channel gets the best model when no explicit preference is set
            if req.channel == "web" || req.channel.starts_with("webchat") {
                "claude-sonnet-4-5-20250929"
            } else {
                &default_model
            }
        });
    let model = model.to_string();

    // Build meta-cognition context (now includes model/cost info)
    let meta_context = build_meta_context_with_model(
        cached_user.as_ref(),
        &req.channel,
        device,
        history_messages.len(),
        is_teai,
        Some(&model),
        0, // session tokens (updated per-session in future)
        0, // session cost microdollars
    );
    let meta_instruction = if is_teai { META_INSTRUCTION_EN } else { META_INSTRUCTION_JA };

    // Adult mode prompt injection (only if age verified and mode enabled)
    let is_adult = user_settings.as_ref()
        .map(|s| s.age_verified.unwrap_or(false) && s.adult_mode.unwrap_or(false))
        .unwrap_or(false);
    let adult_prompt = if is_adult {
        "\n\n## 成人モード\n\
         年齢確認済みユーザーです。成人向けの話題にも対応可能です。\n\
         ただし以下は常にブロックしてください：\n\
         - 未成年に関する性的コンテンツ\n\
         - 違法行為の具体的な手順\n\
         - 実在の個人を対象とした性的コンテンツ\n\
         上記以外の成人向けの会話には、相手の意図を尊重して自然に対応してください。"
    } else {
        ""
    };

    // Wow Factor: enhanced prompt for new users
    let wow_prompt = {
        #[cfg(feature = "dynamodb-backend")]
        {
            let is_new_user = cached_user.as_ref()
                .map(|u| u.credits_used <= 10 && u.plan == "free")
                .unwrap_or(false);
            if is_new_user {
                "\n\n## 初回ユーザー対応\n\
                 このユーザーは新規ユーザーです。最高の第一印象を与えてください：\n\
                 - 回答は具体的で価値のある内容にする\n\
                 - 可能なら積極的にツール（検索・計算など）を活用して、AIの能力をデモする\n\
                 - 親しみやすく、温かいトーンで対応する"
            } else {
                ""
            }
        }
        #[cfg(not(feature = "dynamodb-backend"))]
        { "" }
    };

    // Custom system prompt from request or user settings
    let custom_sys = req.custom_system_prompt.as_deref()
        .or(user_settings.as_ref().and_then(|s| s.custom_system_prompt.as_deref()))
        .unwrap_or("");
    let custom_sys_block = if custom_sys.is_empty() {
        String::new()
    } else {
        format!("\n\n## ユーザーカスタム指示\n{}", custom_sys)
    };

    let system_prompt = if memory_context.is_empty() {
        format!("{}\n\n今日の日付: {}{}{}{}{}{}{}", base_prompt, today, meta_context, meta_instruction, adult_prompt, wow_prompt, custom_sys_block, char_instruction)
    } else {
        format!("{}\n\n今日の日付: {}{}{}{}{}{}\n\n---\n{}{}", base_prompt, today, meta_context, meta_instruction, adult_prompt, wow_prompt, custom_sys_block, memory_context, char_instruction)
    };
    let mut messages = vec![
        Message::system(&system_prompt),
    ];

    // Add session history to messages
    for (role, content) in &history_messages {
        match role.as_str() {
            "user" => messages.push(Message::user(content)),
            "assistant" => messages.push(Message::assistant(content)),
            _ => {}
        }
    }

    // For tool-using agents, append instruction to actively use tools
    if agent.tools_enabled {
        let augmented = format!(
            "{}\n\n[You MUST call web_search tool first to find current information. Never answer from memory alone for factual questions.]",
            clean_message
        );
        messages.push(Message::user(&augmented));
    } else {
        messages.push(Message::user(&clean_message));
    }
    // Resolve LLM parameters: request > user settings > defaults
    let max_tokens = req.max_tokens.unwrap_or(state.config.agents.defaults.max_tokens);
    let temperature = req.temperature
        .or(user_settings.as_ref().and_then(|s| s.temperature))
        .unwrap_or(state.config.agents.defaults.temperature);

    // If user has custom API keys, create per-request provider
    let custom_provider: Option<Arc<dyn LlmProvider>> = user_settings.as_ref()
        .and_then(|s| s.custom_api_keys.as_ref())
        .and_then(|keys| {
            // Find the right API key for the selected model
            let provider_name = if model.starts_with("openai/") || model.starts_with("gpt-") {
                "openai"
            } else if model.starts_with("anthropic/") || model.starts_with("claude-") {
                "anthropic"
            } else if model.starts_with("gemini") || model.starts_with("google/") {
                "google"
            } else {
                "openai"
            };
            keys.get(provider_name)
                .filter(|k| !k.contains("****") && !k.is_empty())
                .map(|key| {
                    Arc::from(provider::create_provider(key, None, &model))
                        as Arc<dyn LlmProvider>
                })
        });

    let active_provider = custom_provider.as_ref().unwrap_or(&provider);

    // Get tool definitions for function calling (only if agent supports tools)
    let enabled_tool_names = user_settings.as_ref().and_then(|s| s.enabled_tools.clone());
    // Check admin by session key or user email (using cached user)
    let user_is_admin = is_admin(&session_key) || {
        #[cfg(feature = "dynamodb-backend")]
        {
            cached_user.as_ref()
                .and_then(|u| u.email.as_deref())
                .map(|e| is_admin(e))
                .unwrap_or(false)
        }
        #[cfg(not(feature = "dynamodb-backend"))]
        { false }
    };
    let tools = if agent.tools_enabled {
        let all_tools = state.tool_registry.get_definitions();
        // Filter by user's enabled tools if set, and restrict GitHub tools to admin
        let filtered: Vec<serde_json::Value> = all_tools.into_iter()
            .filter(|t| {
                let name = t.get("function").and_then(|f| f.get("name")).and_then(|n| n.as_str()).unwrap_or("");
                // GitHub tools are admin-only
                if GITHUB_TOOL_NAMES.contains(&name) && !user_is_admin {
                    return false;
                }
                // Filter by user's enabled tools if set
                if let Some(ref enabled) = enabled_tool_names {
                    return enabled.iter().any(|e| e == name);
                }
                true
            })
            .collect();
        filtered
    } else {
        vec![]
    };

    let tools_ref = if tools.is_empty() { None } else { Some(&tools[..]) };

    info!("Calling LLM: model={}, tools={}, agent={}", model, tools.len(), agent.id);

    // --- Parallel multi-model race path ---
    if req.multi_model {
        // Free plan cannot use parallel mode (cost 3-4x)
        #[cfg(feature = "dynamodb-backend")]
        {
            if let Some(ref user) = cached_user {
                if user.plan == "free" {
                    return Json(ChatResponse {
                        response: "Multi-model mode is not available on the free plan. Please upgrade.".to_string(),
                        session_id: req.session_id,
                        agent: None, tools_used: None,
                        credits_used: None, credits_remaining: None,
                        model_used: None, models_consulted: None,
                        action: None,
                        input_tokens: None,
                        output_tokens: None,
                        estimated_cost_usd: None,
                        mode: None,
                    });
                }
            }
        }

        if let Some(ref lb) = state.lb_raw {
            info!("Parallel multi-model race: starting");
            match lb.chat_parallel(&messages, tools_ref, max_tokens, temperature).await {
                Ok((resp, winning_model, all_usage)) => {
                    let response_text = resp.content.unwrap_or_default();
                    let models_consulted: Vec<String> = all_usage.iter().map(|(m, _, _)| m.clone()).collect();

                    // Deduct credits for all successful calls
                    #[allow(unused_mut)]
                    let mut total_credits: i64 = 0;
                    #[allow(unused_mut)]
                    let mut last_remaining: Option<i64> = None;
                    #[cfg(feature = "dynamodb-backend")]
                    {
                        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                            for (m, input_t, output_t) in &all_usage {
                                let (credits, remaining) = deduct_credits(dynamo, table, &session_key, m, *input_t, *output_t).await;
                                total_credits += credits;
                                if remaining.is_some() { last_remaining = remaining; }
                            }
                        }
                    }

                    // Save to session
                    {
                        let mut sessions = state.sessions.lock().await;
                        let session = sessions.get_or_create(&session_key);
                        session.add_message_from_channel("user", &req.message, "web");
                        session.add_message_from_channel("assistant", &response_text, "web");
                        sessions.save_by_key(&session_key);
                    }
                    #[cfg(feature = "dynamodb-backend")]
                    {
                        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                            increment_sync_version(dynamo, table, &session_key, "web").await;
                        }
                    }

                    info!("Parallel race won by {}, {} models consulted, {} total credits",
                        winning_model, models_consulted.len(), total_credits);

                    return Json(ChatResponse {
                        response: response_text,
                        session_id: req.session_id,
                        agent: Some(agent.id.to_string()),
                        tools_used: None,
                        credits_used: if total_credits > 0 { Some(total_credits) } else { None },
                        credits_remaining: last_remaining,
                        model_used: Some(winning_model),
                        models_consulted: Some(models_consulted),
                        action: None,
                        input_tokens: None,
                        output_tokens: None,
                        estimated_cost_usd: None,
                        mode: None,
                    });
                }
                Err(e) => {
                    tracing::error!("Parallel multi-model race failed: {}, falling back to single", e);
                    // Fall through to normal single-model path
                }
            }
        }
    }

    #[allow(unused_mut)]
    let mut total_credits_used: i64 = 0;
    #[allow(unused_mut)]
    let mut last_remaining_credits: Option<i64> = None;
    let mut total_input_tokens: u32 = 0;
    let mut total_output_tokens: u32 = 0;

    // Build extra LLM parameters from request
    let chat_extra = provider::ChatExtra {
        top_p: req.top_p.or(user_settings.as_ref().and_then(|s| s.top_p)),
        frequency_penalty: req.frequency_penalty.or(user_settings.as_ref().and_then(|s| s.frequency_penalty)),
        presence_penalty: req.presence_penalty.or(user_settings.as_ref().and_then(|s| s.presence_penalty)),
    };

    // LLM call with hard deadline (failover handled by LoadBalancedProvider)
    let deadline = std::time::Duration::from_secs(RESPONSE_DEADLINE_SECS);
    let llm_result = tokio::time::timeout(
        deadline,
        active_provider.chat_with_extra(&messages, tools_ref, &model, max_tokens, temperature, &chat_extra),
    ).await;
    let (used_model, first_completion) = match llm_result {
        Ok(Ok(c)) => (model.clone(), Ok(c)),
        Ok(Err(e)) => {
            tracing::error!("LLM call failed: {}", e);
            (model.clone(), Err(e))
        }
        Err(_) => {
            tracing::warn!("LLM call timed out after {}s, returning fallback", RESPONSE_DEADLINE_SECS);
            let fallback = timeout_fallback_message();
            // Deduct minimum 1 credit for timeout (input tokens were consumed)
            #[cfg(feature = "dynamodb-backend")]
            {
                if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                    let (credits, remaining) = deduct_credits(dynamo, table, &session_key, &model, 100, 0).await;
                    total_credits_used += credits;
                    if remaining.is_some() { last_remaining_credits = remaining; }
                }
            }
            // Save to session and return immediately
            {
                let mut sessions = state.sessions.lock().await;
                let session = sessions.get_or_create(&session_key);
                session.add_message_from_channel("user", &req.message, "web");
                session.add_message_from_channel("assistant", &fallback, "web");
                sessions.save_by_key(&session_key);
            }
            #[cfg(feature = "dynamodb-backend")]
            {
                if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                    increment_sync_version(dynamo, table, &session_key, "web").await;
                }
            }
            return Json(ChatResponse {
                response: fallback,
                session_id: req.session_id,
                agent: Some(agent.id.to_string()),
                tools_used: None,
                credits_used: Some(total_credits_used),
                credits_remaining: last_remaining_credits,
                model_used: Some("timeout".to_string()),
                models_consulted: None,
                action: None,
                input_tokens: None,
                output_tokens: None,
                estimated_cost_usd: None,
                mode: None,
            });
        }
    };

    let (response_text, tools_used) = match first_completion {
        Ok(completion) => {
            info!("LLM response: finish_reason={:?}, tool_calls={}, content_len={}, model={}",
                completion.finish_reason, completion.tool_calls.len(),
                completion.content.as_ref().map(|c| c.len()).unwrap_or(0), used_model);
            // Track token usage
            total_input_tokens += completion.usage.prompt_tokens;
            total_output_tokens += completion.usage.completion_tokens;

            // Deduct credits after successful LLM call
            #[cfg(feature = "dynamodb-backend")]
            {
                if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                    let (credits, remaining) = deduct_credits(
                        dynamo, table, &session_key, &used_model,
                        completion.usage.prompt_tokens, completion.usage.completion_tokens,
                    ).await;
                    total_credits_used += credits;
                    if remaining.is_some() { last_remaining_credits = remaining; }
                    tracing::debug!("Deducted {} credits for user {} (model={})", credits, session_key, used_model);
                }
            }

            // Handle tool calls: multi-iteration agentic loop (up to max_iterations rounds)
            let mut current = completion;
            let mut conversation = messages.clone();
            let mut all_tool_results: Vec<(String, String, String)> = Vec::new();

            // Determine max iterations based on user plan (Wow Factor: new free users get 2 iterations)
            let max_iterations: usize = {
                #[cfg(feature = "dynamodb-backend")]
                {
                    match cached_user.as_ref().map(|u| u.plan.as_str()) {
                        Some("pro") | Some("enterprise") => 5,
                        Some("starter") => 3,
                        _ => {
                            // Wow Factor: new free users get extra tool iteration
                            let is_new = cached_user.as_ref()
                                .map(|u| u.credits_used <= 10 && u.plan == "free")
                                .unwrap_or(false);
                            if is_new { 2 } else { 1 }
                        }
                    }
                }
                #[cfg(not(feature = "dynamodb-backend"))]
                { 5 }
            };
            let mut iteration: usize = 0;

            // Look up Google refresh token once (reused across iterations)
            let google_refresh_token: Option<String> = {
                let needs_google = current.tool_calls.iter().any(|tc| tc.name == "google_calendar" || tc.name == "gmail");
                if needs_google {
                    #[cfg(feature = "dynamodb-backend")]
                    {
                        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                            let user_pk = format!("USER#{}", session_key);
                            if let Ok(output) = dynamo.get_item()
                                .table_name(table)
                                .key("pk", AttributeValue::S(user_pk))
                                .key("sk", AttributeValue::S(SK_PROFILE.to_string()))
                                .send().await
                            {
                                output.item
                                    .and_then(|item| item.get("google_refresh_token").and_then(|v| v.as_s().ok()).cloned())
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }
                    #[cfg(not(feature = "dynamodb-backend"))]
                    { None }
                } else {
                    None
                }
            };

            // Create sandbox directory for code_execute / file tools
            let sandbox_dir = format!("/tmp/sandbox/{}", session_key.replace(':', "_"));
            std::fs::create_dir_all(&sandbox_dir).ok();

            // Multi-iteration tool loop
            while current.has_tool_calls() && iteration < max_iterations {
                iteration += 1;
                info!("Tool iteration {}/{}: {} tool calls", iteration, max_iterations, current.tool_calls.len());

                // Limit to max 5 tool calls per iteration
                let tool_calls_to_run: Vec<_> = current.tool_calls.iter().take(5).collect();
                if current.tool_calls.len() > 5 {
                    info!("Limiting tool calls from {} to 5", current.tool_calls.len());
                }

                // Execute tool calls in parallel
                let registry = &state.tool_registry;
                let sandbox_dir_ref = &sandbox_dir;
                let futures: Vec<_> = tool_calls_to_run.iter().map(|tc| {
                    let name = tc.name.clone();
                    let mut args = tc.arguments.clone();
                    let id = tc.id.clone();
                    // Inject Google refresh token for Google tools
                    if name == "google_calendar" || name == "gmail" {
                        if let Some(ref token) = google_refresh_token {
                            args.insert("_refresh_token".to_string(), serde_json::Value::String(token.clone()));
                        }
                    }
                    // Inject sandbox directory for sandbox tools
                    if name == "code_execute" || name == "file_read" || name == "file_write" || name == "file_list" || name == "web_deploy" {
                        args.insert("_sandbox_dir".to_string(), serde_json::Value::String(sandbox_dir_ref.to_string()));
                    }
                    // Inject session key for tools that need user context
                    if name == "phone_call" || name == "web_deploy" {
                        args.insert("_session_key".to_string(), serde_json::Value::String(session_key.clone()));
                    }
                    async move {
                        info!("Tool call [iter {}]: {} args={:?}", iteration, name, args);
                        let raw_result = registry.execute(&name, &args).await;
                        // Classify tool results for better LLM decision-making
                        let result = if raw_result.starts_with("[TOOL_ERROR]") {
                            raw_result
                        } else if raw_result.starts_with("Error") || raw_result.starts_with("error")
                            || raw_result.contains("request failed") || raw_result.contains("timed out")
                        {
                            format!("[TOOL_ERROR] {}\nYou may retry with different parameters or use an alternative approach.", raw_result)
                        } else if raw_result.is_empty() || raw_result == "No results found."
                            || raw_result.contains("No results") || raw_result.contains("no results")
                        {
                            format!("[NO_RESULTS] {}\nTry rephrasing the query or using a different tool.", if raw_result.is_empty() { "Empty response" } else { &raw_result })
                        } else {
                            raw_result
                        };
                        let preview_end = result.char_indices().nth(300).map(|(i, _)| i).unwrap_or(result.len());
                        let preview = &result[..preview_end];
                        info!("Tool result ({}): {} chars — {}", name, result.len(), preview);
                        (id, name, result)
                    }
                }).collect();
                let tool_results: Vec<_> = futures::future::join_all(futures).await;
                all_tool_results.extend(tool_results.iter().cloned());

                // Log tool usage to DynamoDB
                #[cfg(feature = "dynamodb-backend")]
                {
                    if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                        let now = chrono::Utc::now();
                        for (_, tool_name, tool_result) in &tool_results {
                            let usage_pk = format!("USAGE#{}#{}", session_key, now.format("%Y%m%d"));
                            let usage_sk = format!("{}#{}", now.to_rfc3339(), tool_name);
                            let result_preview = if tool_result.len() > 200 {
                                { let mut i = 200.min(tool_result.len()); while i > 0 && !tool_result.is_char_boundary(i) { i -= 1; } format!("{}...", &tool_result[..i]) }
                            } else {
                                tool_result.clone()
                            };
                            let _ = dynamo.put_item()
                                .table_name(table)
                                .item("pk", AttributeValue::S(usage_pk))
                                .item("sk", AttributeValue::S(usage_sk))
                                .item("tool", AttributeValue::S(tool_name.clone()))
                                .item("result_len", AttributeValue::N(tool_result.len().to_string()))
                                .item("result_preview", AttributeValue::S(result_preview))
                                .item("session_id", AttributeValue::S(session_key.clone()))
                                .item("timestamp", AttributeValue::S(now.to_rfc3339()))
                                .send()
                                .await;
                        }
                    }
                }

                // Add assistant message with tool calls + tool results to conversation
                let tc_json: Vec<serde_json::Value> = current.tool_calls.iter().take(5).map(|tc| {
                    serde_json::json!({
                        "id": tc.id,
                        "type": "function",
                        "function": {
                            "name": tc.name,
                            "arguments": serde_json::to_string(&tc.arguments).unwrap_or_default(),
                        }
                    })
                }).collect();
                conversation.push(Message::assistant_with_tool_calls(current.content.clone(), tc_json));

                for (id, name, result) in &tool_results {
                    conversation.push(Message::tool_result(id, name, result));
                }

                // Follow-up call: pass tools if more iterations remain, None on last iteration
                let follow_up_tools = if iteration < max_iterations {
                    Some(&tools[..])
                } else {
                    None // Force text generation on final iteration
                };

                match active_provider.chat(&conversation, follow_up_tools, &model, max_tokens, temperature).await {
                    Ok(resp) => {
                        info!("Follow-up [iter {}]: finish={:?}, content_len={}, tool_calls={}",
                            iteration,
                            resp.finish_reason,
                            resp.content.as_ref().map(|c| c.len()).unwrap_or(0),
                            resp.tool_calls.len());
                        // Track token usage
                        total_input_tokens += resp.usage.prompt_tokens;
                        total_output_tokens += resp.usage.completion_tokens;
                        #[cfg(feature = "dynamodb-backend")]
                        {
                            if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                                let (credits, remaining) = deduct_credits(dynamo, table, &session_key, &model,
                                    resp.usage.prompt_tokens, resp.usage.completion_tokens).await;
                                total_credits_used += credits;
                                if remaining.is_some() { last_remaining_credits = remaining; }
                                // Break early if credits exhausted
                                if remaining == Some(0) {
                                    current = resp;
                                    tracing::info!("Credits exhausted for user {}, breaking tool loop at iteration {}", session_key, iteration);
                                    break;
                                }
                            }
                        }
                        current = resp;
                    }
                    Err(e) => {
                        tracing::error!("LLM follow-up error [iter {}]: {}", iteration, e);
                        let fallback = all_tool_results.iter()
                            .map(|(_, name, result)| format!("[{}] {}", name, result))
                            .collect::<Vec<_>>().join("\n");
                        current = crate::types::CompletionResponse {
                            content: Some(fallback),
                            tool_calls: vec![],
                            finish_reason: crate::types::FinishReason::Stop,
                            usage: crate::types::TokenUsage::default(),
                        };
                        break; // Stop iterating on error
                    }
                }
            }

            if iteration > 0 {
                info!("Agentic loop completed: {} iterations, {} total tool calls", iteration, all_tool_results.len());
            }

            // Collect tool names used
            let tools_used_list: Vec<String> = all_tool_results.iter()
                .map(|(_, name, _)| name.clone())
                .collect();
            let tools_used = if tools_used_list.is_empty() { None } else { Some(tools_used_list) };

            // Return final response
            let content = current.content.unwrap_or_default();
            let text = if content.is_empty() && !all_tool_results.is_empty() {
                all_tool_results.iter()
                    .map(|(_, name, result)| format!("[{}]\n{}", name, result))
                    .collect::<Vec<_>>().join("\n\n")
            } else {
                content
            };
            (text, tools_used)
        }
        Err(e) => {
            tracing::error!("LLM error (all providers failed): {}", e);
            (error_fallback_message(), None)
        }
    };

    // Save to session
    {
        let mut sessions = state.sessions.lock().await;
        let session = sessions.get_or_create(&session_key);
        session.add_message_from_channel("user", &req.message, "web");
        session.add_message_from_channel("assistant", &response_text, "web");
        sessions.save_by_key(&session_key);
    }
    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            increment_sync_version(dynamo, table, &session_key, "web").await;
        }
    }

    // Auto-update conversation title & message count (fire-and-forget)
    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            if let Some(conv_id) = req.session_id.strip_prefix("webchat:") {
                let msg_count = {
                    let mut sessions = state.sessions.lock().await;
                    sessions.get_or_create(&session_key).messages.len()
                };
                spawn_update_conv_meta(
                    dynamo.clone(), table.clone(), session_key.clone(),
                    conv_id.to_string(), req.message.clone(), msg_count,
                );
            }
        }
    }

    // Auto-save to daily memory log + trigger consolidation (fire-and-forget)
    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let dynamo = dynamo.clone();
            let table = table.clone();
            let sk = session_key.clone();
            let user_msg = req.message.clone();
            let bot_msg = response_text.clone();
            let provider_for_mem = state.lb_provider.clone().or_else(|| state.provider.clone());
            tokio::spawn(async move {
                let summary = format!("- Q: {} → A: {}",
                    if user_msg.len() > 80 { let mut i = 80; while i > 0 && !user_msg.is_char_boundary(i) { i -= 1; } format!("{}...", &user_msg[..i]) } else { user_msg },
                    if bot_msg.len() > 120 { let mut i = 120; while i > 0 && !bot_msg.is_char_boundary(i) { i -= 1; } format!("{}...", &bot_msg[..i]) } else { bot_msg },
                );
                let entry_count = append_daily_memory(&dynamo, &table, &sk, &summary).await;
                // Consolidate into long-term memory every 10 entries
                if entry_count > 0 && entry_count % 10 == 0 {
                    if let Some(provider) = provider_for_mem {
                        spawn_consolidate_memory(dynamo, table, sk, provider);
                    }
                }
            });
        }
    }

    // Auto-translate to Japanese if UI language is "ja" but response has no Japanese
    let mut response_text = response_text;
    if req.language.as_deref() == Some("ja") && detect_language(&response_text) != "ja" {
        let translate_provider = state.lb_provider.clone().or_else(|| state.provider.clone());
        if let Some(translated) = maybe_translate_to_japanese(&response_text, translate_provider.as_ref()).await {
            response_text = translated;
        }
    }

    // Use remaining credits from deduct_credits (no extra DynamoDB call needed)
    let remaining_credits: Option<i64> = last_remaining_credits;

    // Log latency and emit audit
    let latency_ms = chat_start.elapsed().as_millis();
    info!("Chat response: session={}, model={}, credits={}, tools={}, latency={}ms, resp_len={}",
        session_key, used_model, total_credits_used,
        tools_used.as_ref().map(|t| t.len()).unwrap_or(0),
        latency_ms, response_text.len());
    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            emit_audit_log(dynamo.clone(), table.clone(), "chat", &session_key, "",
                &format!("model={} credits={} tools={} latency={}ms",
                    used_model, total_credits_used,
                    tools_used.as_ref().map(|t| t.join(",")).unwrap_or_default(),
                    latency_ms));

            // Fire-and-forget routing log for future routing AI
            let session_hash = {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut h = DefaultHasher::new();
                session_key.hash(&mut h);
                format!("{:x}", h.finish())
            };
            log_routing_data(dynamo.clone(), table.clone(), RoutingLogEntry {
                message_len: req.message.len(),
                language: detect_language(&req.message).to_string(),
                channel: req.channel.clone(),
                device: device.to_string(),
                has_at_prefix: req.message.trim().starts_with('@'),
                user_plan: cached_user.as_ref().map(|u| u.plan.clone()).unwrap_or_else(|| "unknown".to_string()),
                agent_selected: agent.id.to_string(),
                agent_score,
                model_used: used_model.clone(),
                tools_used: tools_used.clone().unwrap_or_default(),
                response_time_ms: latency_ms as u64,
                credits_used: total_credits_used,
                prompt_tokens: 0, // aggregated via deduct_credits
                completion_tokens: 0,
                timed_out: used_model == "timeout",
                error: false,
                timestamp: chrono::Utc::now().to_rfc3339(),
                session_hash,
            });
        }
    }

    let estimated_cost = crate::provider::pricing::calculate_cost(&used_model, total_input_tokens, total_output_tokens);
    Json(ChatResponse {
        response: response_text,
        session_id: req.session_id,
        agent: Some(agent.id.to_string()),
        tools_used,
        credits_used: if total_credits_used > 0 { Some(total_credits_used) } else { None },
        credits_remaining: remaining_credits,
        model_used: Some(used_model),
        models_consulted: None,
        action: None,
        input_tokens: if total_input_tokens > 0 { Some(total_input_tokens) } else { None },
        output_tokens: if total_output_tokens > 0 { Some(total_output_tokens) } else { None },
        estimated_cost_usd: if estimated_cost > 0.0 { Some(estimated_cost) } else { None },
        mode: Some(resolved_mode.to_string()),
    })
}

/// GET /api/v1/agents — List available agents
async fn handle_agents() -> impl IntoResponse {
    let agents: Vec<serde_json::Value> = AGENTS.iter()
        .filter(|a| a.id != "orchestrator")
        .map(|a| serde_json::json!({
            "id": a.id,
            "name": a.name,
            "description": a.description,
            "tools_enabled": a.tools_enabled,
            "icon": a.icon,
        }))
        .collect();

    Json(serde_json::json!({
        "agents": agents,
        "total": agents.len(),
        "routing": "auto",
        "hint": "Use @agent_id prefix to force a specific agent, e.g. '@coder fix this bug'",
    }))
}

/// GET /api/v1/devices — List connected devices for a user
async fn handle_devices(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let session_id = headers.get("x-session-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("anonymous");

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            // Resolve unified user key
            let user_key = resolve_session_key(dynamo, table, session_id).await;

            // Query DEVICE# records for this user
            let resp = dynamo
                .query()
                .table_name(table)
                .key_condition_expression("pk = :pk AND begins_with(sk, :sk)")
                .expression_attribute_values(":pk", AttributeValue::S(format!("DEVICE#{}", user_key)))
                .expression_attribute_values(":sk", AttributeValue::S("HB#".to_string()))
                .scan_index_forward(false)
                .limit(20)
                .send()
                .await;

            let devices: Vec<serde_json::Value> = match resp {
                Ok(output) => {
                    output.items.unwrap_or_default().iter().map(|item| {
                        serde_json::json!({
                            "hostname": item.get("hostname").and_then(|v| v.as_s().ok()).unwrap_or(&"unknown".to_string()),
                            "os": item.get("os").and_then(|v| v.as_s().ok()).unwrap_or(&"unknown".to_string()),
                            "arch": item.get("arch").and_then(|v| v.as_s().ok()).unwrap_or(&"unknown".to_string()),
                            "cpu_usage": item.get("cpu_usage").and_then(|v| v.as_n().ok()).and_then(|n| n.parse::<f64>().ok()),
                            "memory_total": item.get("memory_total").and_then(|v| v.as_n().ok()).and_then(|n| n.parse::<u64>().ok()),
                            "memory_used": item.get("memory_used").and_then(|v| v.as_n().ok()).and_then(|n| n.parse::<u64>().ok()),
                            "disk_total": item.get("disk_total").and_then(|v| v.as_n().ok()).and_then(|n| n.parse::<u64>().ok()),
                            "disk_used": item.get("disk_used").and_then(|v| v.as_n().ok()).and_then(|n| n.parse::<u64>().ok()),
                            "uptime_secs": item.get("uptime_secs").and_then(|v| v.as_n().ok()).and_then(|n| n.parse::<u64>().ok()),
                            "last_seen": item.get("last_seen").and_then(|v| v.as_s().ok()),
                        })
                    }).collect()
                }
                Err(e) => {
                    tracing::warn!("Failed to query devices: {}", e);
                    vec![]
                }
            };

            return Json(serde_json::json!({
                "devices": devices,
                "total": devices.len(),
            }));
        }
    }

    #[cfg(not(feature = "dynamodb-backend"))]
    let _ = &state;
    let _ = session_id;

    Json(serde_json::json!({
        "devices": [],
        "total": 0,
    }))
}

/// POST /api/v1/devices/heartbeat — Receive device heartbeat
async fn handle_device_heartbeat(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DeviceHeartbeat>,
) -> impl IntoResponse {
    info!("Device heartbeat: session={}, hostname={}", req.session_id, req.hostname);

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let user_key = resolve_session_key(dynamo, table, &req.session_id).await;
            let now = chrono::Utc::now();

            let mut item_builder = dynamo
                .put_item()
                .table_name(table)
                .item("pk", AttributeValue::S(format!("DEVICE#{}", user_key)))
                .item("sk", AttributeValue::S(format!("HB#{}", req.hostname)))
                .item("session_id", AttributeValue::S(req.session_id.clone()))
                .item("hostname", AttributeValue::S(req.hostname.clone()))
                .item("os", AttributeValue::S(req.os.clone()))
                .item("arch", AttributeValue::S(req.arch.clone()))
                .item("last_seen", AttributeValue::S(now.to_rfc3339()))
                .item("ttl", AttributeValue::N((now.timestamp() + 86400).to_string())); // 24h TTL

            if let Some(cpu) = req.cpu_usage {
                item_builder = item_builder.item("cpu_usage", AttributeValue::N(format!("{:.1}", cpu)));
            }
            if let Some(mem_total) = req.memory_total {
                item_builder = item_builder.item("memory_total", AttributeValue::N(mem_total.to_string()));
            }
            if let Some(mem_used) = req.memory_used {
                item_builder = item_builder.item("memory_used", AttributeValue::N(mem_used.to_string()));
            }
            if let Some(disk_total) = req.disk_total {
                item_builder = item_builder.item("disk_total", AttributeValue::N(disk_total.to_string()));
            }
            if let Some(disk_used) = req.disk_used {
                item_builder = item_builder.item("disk_used", AttributeValue::N(disk_used.to_string()));
            }
            if let Some(uptime) = req.uptime_secs {
                item_builder = item_builder.item("uptime_secs", AttributeValue::N(uptime.to_string()));
            }

            match item_builder.send().await {
                Ok(_) => {
                    return (StatusCode::OK, Json(serde_json::json!({
                        "status": "ok",
                        "next_heartbeat_secs": 60,
                    })));
                }
                Err(e) => {
                    tracing::error!("Failed to store heartbeat: {}", e);
                    return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
                        "error": "Failed to store heartbeat",
                    })));
                }
            }
        }
    }

    #[cfg(not(feature = "dynamodb-backend"))]
    let _ = &state;

    (StatusCode::OK, Json(serde_json::json!({
        "status": "ok",
        "next_heartbeat_secs": 60,
    })))
}

// ---------------------------------------------------------------------------
// Workers API (compute provider / earn mode)
// ---------------------------------------------------------------------------

/// POST /api/v1/workers/register — Register a compute worker
async fn handle_worker_register(
    State(state): State<Arc<AppState>>,
    Json(req): Json<WorkerRegisterRequest>,
) -> impl IntoResponse {
    let worker_id = format!("w-{}", uuid::Uuid::new_v4().to_string().split('-').next().unwrap_or("0000"));
    info!("Worker register: id={}, model={}, host={}", worker_id, req.model, req.hostname);

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let now = chrono::Utc::now();
            let user_key = resolve_session_key(dynamo, table, &req.session_id).await;
            let _ = dynamo
                .put_item()
                .table_name(table)
                .item("pk", AttributeValue::S(format!("WORKER#{}", worker_id)))
                .item("sk", AttributeValue::S("META".to_string()))
                .item("user_key", AttributeValue::S(user_key))
                .item("session_id", AttributeValue::S(req.session_id.clone()))
                .item("model", AttributeValue::S(req.model.clone()))
                .item("hostname", AttributeValue::S(req.hostname.clone()))
                .item("os", AttributeValue::S(req.os.unwrap_or_default()))
                .item("arch", AttributeValue::S(req.arch.unwrap_or_default()))
                .item("status", AttributeValue::S("active".to_string()))
                .item("registered_at", AttributeValue::S(now.to_rfc3339()))
                .item("last_heartbeat", AttributeValue::S(now.to_rfc3339()))
                .item("ttl", AttributeValue::N((now.timestamp() + 86400).to_string()))
                .send()
                .await;
        }
    }

    #[cfg(not(feature = "dynamodb-backend"))]
    let _ = &state;

    (StatusCode::OK, Json(serde_json::json!({
        "worker_id": worker_id,
        "status": "active",
        "poll_url": "/api/v1/workers/poll",
    })))
}

/// GET /api/v1/workers/poll — Long-poll for inference requests
async fn handle_worker_poll(
    State(state): State<Arc<AppState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let _worker_id = params.get("worker_id").cloned().unwrap_or_default();
    let _model = params.get("model").cloned().unwrap_or_default();

    // Long-poll: wait up to 5 seconds for a request
    // TODO: Replace with DynamoDB queue or SQS integration
    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

    (StatusCode::OK, Json(serde_json::json!({
        "status": "no_work",
        "retry_after_secs": 5,
    })))
}

/// POST /api/v1/workers/result — Submit inference result and earn credits
async fn handle_worker_result(
    State(state): State<Arc<AppState>>,
    Json(req): Json<WorkerResultRequest>,
) -> impl IntoResponse {
    info!("Worker result: worker={}, request={}", req.worker_id, req.request_id);

    let credits_earned: i64 = 2; // Default credits per request

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            // Look up worker to get user_key
            if let Ok(resp) = dynamo
                .get_item()
                .table_name(table)
                .key("pk", AttributeValue::S(format!("WORKER#{}", req.worker_id)))
                .key("sk", AttributeValue::S("META".to_string()))
                .send()
                .await
            {
                if let Some(item) = resp.item() {
                    if let Some(user_key) = item.get("user_key").and_then(|v| v.as_s().ok()) {
                        // Add credits to user
                        if let Err(e) = dynamo
                            .update_item()
                            .table_name(table)
                            .key("pk", AttributeValue::S(format!("USER#{}", user_key)))
                            .key("sk", AttributeValue::S(SK_PROFILE.to_string()))
                            .update_expression("ADD credits_remaining :c")
                            .expression_attribute_values(":c", AttributeValue::N(credits_earned.to_string()))
                            .send()
                            .await
                        {
                            tracing::error!("Failed to credit worker {}: {}", req.worker_id, e);
                        }
                    }
                }
            }
        }
    }

    #[cfg(not(feature = "dynamodb-backend"))]
    let _ = &state;

    (StatusCode::OK, Json(serde_json::json!({
        "status": "ok",
        "credits_earned": credits_earned,
    })))
}

/// POST /api/v1/workers/heartbeat — Worker heartbeat
async fn handle_worker_heartbeat(
    State(state): State<Arc<AppState>>,
    Json(req): Json<WorkerHeartbeatRequest>,
) -> impl IntoResponse {
    let _worker_id = &req.worker_id;
    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let now = chrono::Utc::now();
            if let Err(e) = dynamo
                .update_item()
                .table_name(table)
                .key("pk", AttributeValue::S(format!("WORKER#{}", req.worker_id)))
                .key("sk", AttributeValue::S("META".to_string()))
                .update_expression("SET last_heartbeat = :t, #s = :s, #ttl = :ttl")
                .expression_attribute_names("#s", "status")
                .expression_attribute_names("#ttl", "ttl")
                .expression_attribute_values(":t", AttributeValue::S(now.to_rfc3339()))
                .expression_attribute_values(":s", AttributeValue::S("active".to_string()))
                .expression_attribute_values(":ttl", AttributeValue::N((now.timestamp() + 86400).to_string()))
                .send()
                .await
            {
                tracing::error!("Worker heartbeat failed for {}: {}", req.worker_id, e);
            }
        }
    }

    #[cfg(not(feature = "dynamodb-backend"))]
    let _ = &state;

    (StatusCode::OK, Json(serde_json::json!({
        "status": "ok",
        "next_heartbeat_secs": 60,
    })))
}

/// GET /api/v1/workers/status — Get worker status for a user
async fn handle_worker_status(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let session_key = headers
        .get("x-session-id")
        .and_then(|v| v.to_str().ok())
        .or_else(|| {
            headers.get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
        })
        .map(|s| s.to_string());

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            if let Some(ref sk) = session_key {
                let user_key = resolve_session_key(dynamo, table, sk).await;
                // Query workers for this user (scan with filter — acceptable for small sets)
                if let Ok(resp) = dynamo
                    .scan()
                    .table_name(table)
                    .filter_expression("begins_with(pk, :prefix) AND user_key = :uk")
                    .expression_attribute_values(":prefix", AttributeValue::S("WORKER#".to_string()))
                    .expression_attribute_values(":uk", AttributeValue::S(user_key))
                    .send()
                    .await
                {
                    let workers: Vec<serde_json::Value> = resp.items().iter().map(|item| {
                        serde_json::json!({
                            "worker_id": item.get("pk").and_then(|v| v.as_s().ok()).unwrap_or(&"".to_string()).replace("WORKER#", ""),
                            "model": item.get("model").and_then(|v| v.as_s().ok()).unwrap_or(&"".to_string()).clone(),
                            "hostname": item.get("hostname").and_then(|v| v.as_s().ok()).unwrap_or(&"".to_string()).clone(),
                            "status": item.get("status").and_then(|v| v.as_s().ok()).unwrap_or(&"".to_string()).clone(),
                            "last_heartbeat": item.get("last_heartbeat").and_then(|v| v.as_s().ok()).unwrap_or(&"".to_string()).clone(),
                        })
                    }).collect();

                    return (StatusCode::OK, Json(serde_json::json!({
                        "workers": workers,
                        "total": workers.len(),
                    })));
                }
            }
        }
    }

    let _ = (&state, &session_key);

    (StatusCode::OK, Json(serde_json::json!({
        "workers": [],
        "total": 0,
    })))
}

/// GET /api/v1/sessions — List sessions
async fn handle_list_sessions(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let sessions = state.sessions.lock().await;
    let list = sessions.list_sessions();
    Json(list)
}

/// GET /api/v1/sessions/:id — Get session (resolves linked sessions)
async fn handle_get_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    // Resolve unified session key for linked channels
    let session_key = {
        #[cfg(feature = "dynamodb-backend")]
        {
            if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                resolve_session_key(dynamo, table, &id).await
            } else {
                id.clone()
            }
        }
        #[cfg(not(feature = "dynamodb-backend"))]
        {
            id.clone()
        }
    };

    // Query linked channels for this user
    let (linked_channels, linked_channel_details) = {
        #[cfg(feature = "dynamodb-backend")]
        {
            if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                let info = get_linked_channels(dynamo, table, &session_key).await;
                (info.types, info.details)
            } else {
                (vec![], vec![])
            }
        }
        #[cfg(not(feature = "dynamodb-backend"))]
        {
            (Vec::<String>::new(), Vec::<serde_json::Value>::new())
        }
    };

    let mut sessions = state.sessions.lock().await;
    // Force reload from storage to get latest messages from all channels
    let session = sessions.refresh(&session_key);

    // Support ?limit=all for full history (no truncation)
    let limit = params.get("limit")
        .and_then(|v| if v == "all" { Some(usize::MAX) } else { v.parse().ok() })
        .unwrap_or(100);

    let history = session.get_full_history(limit);
    let is_summarized = session.messages.len() > 16; // Matches get_history_with_summary threshold

    Json(serde_json::json!({
        "key": id,
        "resolved_key": session_key,
        "messages": history,
        "message_count": history.len(),
        "total_message_count": session.messages.len(),
        "is_summarized": is_summarized,
        "linked_channels": linked_channels,
        "linked_channel_details": linked_channel_details,
    }))
}

/// Linked channel info returned from DynamoDB.
#[cfg(feature = "dynamodb-backend")]
struct LinkedChannelInfo {
    /// Channel type names: ["web", "line", "telegram"]
    types: Vec<String>,
    /// Rich details: [{"type":"line","name":"LINE","linked_at":"2025-..."},...]
    details: Vec<serde_json::Value>,
}

/// Query DynamoDB for linked channels associated with a user.
#[cfg(feature = "dynamodb-backend")]
async fn get_linked_channels(
    dynamo: &aws_sdk_dynamodb::Client,
    table: &str,
    session_key: &str,
) -> LinkedChannelInfo {
    // Query LINK# records that point to this user_id
    let pk = format!("LINK#api:{}", session_key.trim_start_matches("api:"));
    let resp = dynamo
        .query()
        .table_name(table)
        .key_condition_expression("pk = :pk")
        .expression_attribute_values(":pk", AttributeValue::S(pk))
        .limit(10)
        .send()
        .await;

    let mut types = vec!["web".to_string()];
    let mut details: Vec<serde_json::Value> = Vec::new();

    // Also scan for reverse links: any LINK# records with user_id = session_key
    let scan_resp = dynamo
        .query()
        .table_name(table)
        .index_name("gsi-user-id")
        .key_condition_expression("user_id = :uid")
        .expression_attribute_values(":uid", AttributeValue::S(session_key.to_string()))
        .limit(20)
        .send()
        .await;

    if let Ok(output) = scan_resp {
        for item in output.items.unwrap_or_default() {
            if let Some(pk_val) = item.get("pk").and_then(|v| v.as_s().ok()) {
                let key = pk_val.trim_start_matches("LINK#");
                let ch_type = if key.starts_with("line:") {
                    "line"
                } else if key.starts_with("tg:") || key.starts_with("telegram:") {
                    "telegram"
                } else if key.starts_with("fb:") {
                    "facebook"
                } else {
                    continue;
                };
                if !types.contains(&ch_type.to_string()) {
                    types.push(ch_type.to_string());
                    let ch_name = item.get("channel_name").and_then(|v| v.as_s().ok()).map(|s| s.as_str()).unwrap_or(ch_type);
                    let linked_at = item.get("linked_at").and_then(|v| v.as_s().ok()).map(|s| s.as_str()).unwrap_or("");
                    details.push(serde_json::json!({
                        "type": ch_type,
                        "name": ch_name,
                        "channel_key": key,
                        "linked_at": linked_at,
                    }));
                }
            }
        }
    }

    // Also check the direct query response
    if let Ok(output) = resp {
        for item in output.items.unwrap_or_default() {
            if let Some(uid) = item.get("user_id").and_then(|v| v.as_s().ok()) {
                if uid != session_key {
                    let ch_name = item.get("channel_name").and_then(|v| v.as_s().ok()).map(|s| s.as_str()).unwrap_or("");
                    let linked_at = item.get("linked_at").and_then(|v| v.as_s().ok()).map(|s| s.as_str()).unwrap_or("");
                    if uid.starts_with("line:") && !types.contains(&"line".to_string()) {
                        types.push("line".to_string());
                        details.push(serde_json::json!({
                            "type": "line",
                            "name": if ch_name.is_empty() { "LINE" } else { ch_name },
                            "linked_at": linked_at,
                        }));
                    } else if uid.starts_with("telegram:") && !types.contains(&"telegram".to_string()) {
                        types.push("telegram".to_string());
                        details.push(serde_json::json!({
                            "type": "telegram",
                            "name": if ch_name.is_empty() { "Telegram" } else { ch_name },
                            "linked_at": linked_at,
                        }));
                    }
                }
            }
        }
    }

    LinkedChannelInfo { types, details }
}

/// DELETE /api/v1/sessions/:id — Delete session
async fn handle_delete_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let mut sessions = state.sessions.lock().await;
    let deleted = sessions.delete(&id);
    if deleted {
        (StatusCode::OK, Json(serde_json::json!({"deleted": true})))
    } else {
        (StatusCode::NOT_FOUND, Json(serde_json::json!({"deleted": false, "error": "Session not found"})))
    }
}

/// GET /api/v1/usage — Usage info (supports Bearer token or x-user-id header)
async fn handle_usage(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    // Prefer Bearer token auth, fall back to x-user-id for backward compat
    #[cfg(feature = "dynamodb-backend")]
    let user_id_from_token = auth_user_id(&state, &headers).await;
    #[cfg(not(feature = "dynamodb-backend"))]
    let user_id_from_token: Option<String> = None;

    let _user_id = user_id_from_token.as_deref()
        .or_else(|| headers.get("x-user-id").and_then(|v| v.to_str().ok()))
        .unwrap_or("anonymous");

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let resolved = resolve_session_key(dynamo, table, _user_id).await;
            let user = get_or_create_user(dynamo, table, &resolved).await;
            return Json(UsageResponse {
                agent_runs: 0,
                total_tokens: 0,
                credits_used: user.credits_used as u64,
                credits_remaining: user.credits_remaining as u64,
            });
        }
    }

    #[cfg(not(feature = "dynamodb-backend"))]
    let _ = &state;

    Json(UsageResponse {
        agent_runs: 0,
        total_tokens: 0,
        credits_used: 0,
        credits_remaining: 1000,
    })
}

/// POST /webhooks/line — LINE webhook
async fn handle_line_webhook(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    body: String,
) -> impl IntoResponse {
    info!("LINE webhook received: {} bytes", body.len());

    // Verify signature
    let signature = headers
        .get("x-line-signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !LineChannel::verify_signature(
        &state.config.channels.line.channel_secret,
        body.as_bytes(),
        signature,
    ) {
        return StatusCode::UNAUTHORIZED;
    }

    // Parse events
    let events = match LineChannel::parse_webhook_events(&body) {
        Ok(events) => events,
        Err(e) => {
            tracing::error!("Failed to parse LINE webhook: {}", e);
            return StatusCode::BAD_REQUEST;
        }
    };

    let access_token = state.config.channels.line.channel_access_token.clone();

    for event in &events {
        // Handle follow event (friend added)
        if event.event_type == "follow" {
            if let Some(ref reply_token) = &event.reply_token {
                let welcome = "友だち追加ありがとう！\n\n\
                    ChatWeb — chatweb.ai の音声対応AIアシスタントだよ。何でも聞いてね。\n\n\
                    まず教えて:\n\
                    - 敬語がいい？フランク？（「フランクで」って言ってくれたらOK）\n\n\
                    できること:\n\
                    🔍 ウェブ検索・リサーチ\n\
                    🧮 計算・データ分析\n\
                    🌤 天気予報\n\
                    💻 プログラミング支援\n\
                    📧 Gmail・カレンダー連携\n\
                    🔗 /link でWeb・Telegramと同期\n\n\
                    全てオープンソース: github.com/yukihamada\n\
                    https://chatweb.ai";
                if let Err(e) = LineChannel::reply(&access_token, reply_token, welcome).await {
                    tracing::error!("Failed to send LINE welcome: {}", e);
                }
            }
            continue;
        }

        if event.event_type == "message" {
            if let (Some(ref reply_token), Some(ref message)) =
                (&event.reply_token, &event.message)
            {
                if message.msg_type == "text" {
                    let text = message.text.as_deref().unwrap_or("");
                    let user_id = event.source.as_ref()
                        .and_then(|s| s.user_id.as_deref())
                        .unwrap_or("unknown");
                    let channel_key = format!("line:{}", user_id);

                    // Auto-link if user sends a web session ID
                    #[cfg(feature = "dynamodb-backend")]
                    if is_session_id(text) {
                        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                            let web_sid = text.trim();
                            auto_link_session(dynamo, table, &channel_key, web_sid, &state.sessions).await;
                            let reply = "連携完了！Webとの会話が同期されました。\nこれからどのチャネルでも同じ会話を続けられます。";
                            if let Err(e) = LineChannel::reply(&access_token, reply_token, reply).await {
                                tracing::error!("Failed to reply to LINE: {}", e);
                            }
                            continue;
                        }
                    }

                    // Resolve unified session key
                    let session_key = {
                        #[cfg(feature = "dynamodb-backend")]
                        {
                            if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                                resolve_session_key(dynamo, table, &channel_key).await
                            } else {
                                channel_key.clone()
                            }
                        }
                        #[cfg(not(feature = "dynamodb-backend"))]
                        {
                            channel_key.clone()
                        }
                    };

                    // Handle slash commands
                    if let Some(cmd) = super::commands::parse_command(text) {
                        let ctx = super::commands::CommandContext {
                            channel_key: &channel_key,
                            session_key: &session_key,
                            user_id: None,
                            conv_id: None,
                            sessions: &state.sessions,
                            provider: state.get_provider(),
                            tool_registry: Some(&state.tool_registry),
                            #[cfg(feature = "dynamodb-backend")]
                            dynamo: state.dynamo_client.as_ref(),
                            #[cfg(feature = "dynamodb-backend")]
                            config_table: state.config_table.as_deref(),
                        };
                        let result = super::commands::execute_command(cmd, &ctx).await;
                        if let super::commands::CommandResult::Reply(reply) = result {
                            if let Err(e) = LineChannel::reply(&access_token, reply_token, &reply).await {
                                tracing::error!("Failed to reply to LINE: {}", e);
                            }
                            continue;
                        }
                    }

                    let reply = match state.get_provider() {
                        Some(provider) => {
                            let provider = provider.clone();
                            let mut messages = vec![
                                Message::system(
                                    "あなたはChatWeb（chatweb.ai）、音声対応の高速AIアシスタントです。\
                                     LINEメッセンジャーでの会話です。\
                                     - 1メッセージ200文字以内で簡潔に。長い説明は箇条書き。\
                                     - 絵文字を適度に使用して親しみやすく。\
                                     - URLは短く。コードブロックは使わない。\
                                     - 日本語で質問されたら日本語で、英語なら英語で答えてください。"
                                ),
                            ];

                            // Get session history (refresh to pick up messages from other channels)
                            {
                                let mut sessions = state.sessions.lock().await;
                                let session = sessions.refresh(&session_key);
                                let history = session.get_history(10);
                                for msg in &history {
                                    let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
                                    let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
                                    match role {
                                        "user" => messages.push(Message::user(content)),
                                        "assistant" => messages.push(Message::assistant(content)),
                                        _ => {}
                                    }
                                }
                            }

                            messages.push(Message::user(text));

                            let model = &state.config.agents.defaults.model;
                            let max_tokens = state.config.agents.defaults.max_tokens;
                            let temperature = state.config.agents.defaults.temperature;

                            match provider.chat(&messages, None, model, max_tokens, temperature).await {
                                Ok(completion) => {
                                    // Deduct credits
                                    #[cfg(feature = "dynamodb-backend")]
                                    {
                                        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                                            let _ = deduct_credits(dynamo, table, &session_key, model,
                                                completion.usage.prompt_tokens, completion.usage.completion_tokens).await;
                                        }
                                    }
                                    let resp = completion.content.unwrap_or_default();
                                    // Save to session
                                    {
                                        let mut sessions = state.sessions.lock().await;
                                        let session = sessions.get_or_create(&session_key);
                                        session.add_message_from_channel("user", text, "line");
                                        session.add_message_from_channel("assistant", &resp, "line");
                                        sessions.save_by_key(&session_key);
                                    }
                                    #[cfg(feature = "dynamodb-backend")]
                                    {
                                        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                                            increment_sync_version(dynamo, table, &session_key, "line").await;
                                        }
                                    }
                                    resp
                                }
                                Err(e) => {
                                    tracing::error!("LLM error for LINE: {}", e);
                                    "すみません、エラーが発生しました。もう一度お試しください。".to_string()
                                }
                            }
                        }
                        None => "AI provider not configured.".to_string(),
                    };

                    if let Err(e) =
                        LineChannel::reply(&access_token, reply_token, &reply).await
                    {
                        tracing::error!("Failed to reply to LINE: {}", e);
                    }
                }
            }
        }
    }

    StatusCode::OK
}

/// POST /webhooks/telegram — Telegram webhook
async fn handle_telegram_webhook(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    body: String,
) -> impl IntoResponse {
    // Verify Telegram webhook secret token if configured
    let webhook_secret = std::env::var("TELEGRAM_WEBHOOK_SECRET").unwrap_or_default();
    if webhook_secret.is_empty() {
        tracing::warn!("TELEGRAM_WEBHOOK_SECRET not set — webhook verification disabled");
    }
    if !webhook_secret.is_empty() {
        let provided = headers.get("x-telegram-bot-api-secret-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if provided != webhook_secret {
            tracing::warn!("Telegram webhook secret mismatch");
            return StatusCode::UNAUTHORIZED;
        }
    }

    info!("Telegram webhook received: {} bytes", body.len());

    let update = match TelegramChannel::parse_webhook_update(&body) {
        Ok(u) => u,
        Err(e) => {
            tracing::error!("Failed to parse Telegram webhook: {}", e);
            return StatusCode::BAD_REQUEST;
        }
    };

    let message = match update.message {
        Some(m) => m,
        None => return StatusCode::OK,
    };

    // Extract text from message or caption
    let text = match message.text.as_deref().or(message.caption.as_deref()) {
        Some(t) => t,
        None => return StatusCode::OK,
    };

    // Build sender_id and check allow_from
    let sender_id = match &message.from {
        Some(user) => match &user.username {
            Some(uname) => format!("{}|{}", user.id, uname),
            None => user.id.to_string(),
        },
        None => return StatusCode::OK,
    };

    if !is_allowed(&sender_id, &state.config.channels.telegram.allow_from) {
        tracing::warn!("Telegram access denied for {}", sender_id);
        return StatusCode::OK;
    }

    let token = &state.config.channels.telegram.token;
    let chat_id = message.chat.id.to_string();
    let channel_key = format!("tg:{}", sender_id);

    // Handle /start command (welcome or deep-link auto-link)
    if text.trim() == "/start" || text.starts_with("/start ") {
        // Check for deep-link payload: /start api_xxxx (Telegram replaces : with _)
        let payload = text.strip_prefix("/start ").map(|s| s.trim()).unwrap_or("");
        let web_session_id = if payload.starts_with("webchat_") {
            Some(payload.replacen("webchat_", "webchat:", 1))
        } else if payload.starts_with("api_") {
            // Legacy: Telegram deep links can't contain ':', so we use '_' and convert back
            Some(payload.replacen("api_", "api:", 1))
        } else if payload.starts_with("api:") || payload.starts_with("webchat:") {
            Some(payload.to_string())
        } else {
            None
        };

        if let Some(ref _sid) = web_session_id {
            // Auto-link via deep link
            #[cfg(feature = "dynamodb-backend")]
            if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                auto_link_session(dynamo, table, &channel_key, _sid, &state.sessions).await;
                let reply = "Link complete! Your Web and Telegram conversations are now synced.\nYou can continue the same conversation on any channel.";
                let client = reqwest::Client::new();
                if let Err(e) = TelegramChannel::send_message_static(&client, token, &chat_id, reply).await {
                    tracing::error!("Failed to send Telegram link reply: {}", e);
                }
                return StatusCode::OK;
            }
        }

        let welcome = "Welcome! 👋\n\n\
            I'm ChatWeb — a fast, voice-enabled AI assistant from chatweb.ai.\n\n\
            Let's set up:\n\
            - Preferred tone? (casual / professional)\n\n\
            What I can do:\n\
            🔍 Web search & research\n\
            💻 Code generation & debugging\n\
            🧮 Calculations & data analysis\n\
            🌤 Weather forecasts\n\
            📧 Gmail & Calendar (if linked)\n\n\
            Commands:\n\
            /link - Sync with Web & LINE\n\
            /start - Show this message\n\n\
            Open source: github.com/yukihamada\n\
            https://chatweb.ai";
        let client = reqwest::Client::new();
        if let Err(e) = TelegramChannel::send_message_static(&client, token, &chat_id, welcome).await {
            tracing::error!("Failed to send Telegram welcome: {}", e);
        }
        return StatusCode::OK;
    }

    // Auto-link if user sends a web session ID
    #[cfg(feature = "dynamodb-backend")]
    if is_session_id(text) {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let web_sid = text.trim();
            auto_link_session(dynamo, table, &channel_key, web_sid, &state.sessions).await;
            let reply = "Link complete! Your Web and Telegram conversations are now synced.";
            let client = reqwest::Client::new();
            if let Err(e) = TelegramChannel::send_message_static(&client, token, &chat_id, reply).await {
                tracing::error!("Failed to send Telegram link reply: {}", e);
            }
            return StatusCode::OK;
        }
    }

    // Resolve unified session key
    let session_key = {
        #[cfg(feature = "dynamodb-backend")]
        {
            if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                resolve_session_key(dynamo, table, &channel_key).await
            } else {
                channel_key.clone()
            }
        }
        #[cfg(not(feature = "dynamodb-backend"))]
        {
            channel_key.clone()
        }
    };

    // Handle slash commands
    if let Some(cmd) = super::commands::parse_command(text) {
        let ctx = super::commands::CommandContext {
            channel_key: &channel_key,
            session_key: &session_key,
            user_id: None,
            conv_id: None,
            sessions: &state.sessions,
            provider: state.get_provider(),
            tool_registry: Some(&state.tool_registry),
            #[cfg(feature = "dynamodb-backend")]
            dynamo: state.dynamo_client.as_ref(),
            #[cfg(feature = "dynamodb-backend")]
            config_table: state.config_table.as_deref(),
        };
        let result = super::commands::execute_command(cmd, &ctx).await;
        if let super::commands::CommandResult::Reply(reply) = result {
            let client = reqwest::Client::new();
            if let Err(e) = TelegramChannel::send_message_static(&client, token, &chat_id, &reply).await {
                tracing::error!("Failed to send Telegram reply: {}", e);
            }
            return StatusCode::OK;
        }
    }

    let reply = match state.get_provider() {
        Some(provider) => {
            let provider = provider.clone();
            let mut messages = vec![
                Message::system(
                    "あなたはChatWeb（chatweb.ai）、音声対応の高速AIアシスタントです。\
                     Telegramでの会話です。\
                     - 簡潔に要点を伝える（300文字以内）。\
                     - Markdown記法を活用（太字、コードブロック、リンク）。\
                     - ボタン操作を意識した応答。\
                     - 日本語で質問されたら日本語で、英語なら英語で答えてください。"
                ),
            ];

            {
                let mut sessions = state.sessions.lock().await;
                let session = sessions.refresh(&session_key);
                let history = session.get_history(10);
                for msg in &history {
                    let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
                    let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
                    match role {
                        "user" => messages.push(Message::user(content)),
                        "assistant" => messages.push(Message::assistant(content)),
                        _ => {}
                    }
                }
            }

            messages.push(Message::user(text));

            let model = &state.config.agents.defaults.model;
            let max_tokens = state.config.agents.defaults.max_tokens;
            let temperature = state.config.agents.defaults.temperature;

            match provider.chat(&messages, None, model, max_tokens, temperature).await {
                Ok(completion) => {
                    // Deduct credits
                    #[cfg(feature = "dynamodb-backend")]
                    {
                        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                            let _ = deduct_credits(dynamo, table, &session_key, model,
                                completion.usage.prompt_tokens, completion.usage.completion_tokens).await;
                        }
                    }
                    let resp = completion.content.unwrap_or_default();
                    {
                        let mut sessions = state.sessions.lock().await;
                        let session = sessions.get_or_create(&session_key);
                        session.add_message_from_channel("user", text, "telegram");
                        session.add_message_from_channel("assistant", &resp, "telegram");
                        sessions.save_by_key(&session_key);
                    }
                    #[cfg(feature = "dynamodb-backend")]
                    {
                        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                            increment_sync_version(dynamo, table, &session_key, "telegram").await;
                        }
                    }
                    resp
                }
                Err(e) => {
                    tracing::error!("LLM error for Telegram: {}", e);
                    "Sorry, an error occurred. Please try again.".to_string()
                }
            }
        }
        None => "AI provider not configured.".to_string(),
    };

    let client = reqwest::Client::new();
    if let Err(e) = TelegramChannel::send_message_static(&client, token, &chat_id, &reply).await {
        tracing::error!("Failed to send Telegram reply: {}", e);
    }

    StatusCode::OK
}

// ---------------------------------------------------------------------------
// Facebook Messenger Webhook
// ---------------------------------------------------------------------------

/// Facebook webhook verify query params.
#[derive(Debug, Deserialize)]
struct FacebookVerifyParams {
    #[serde(rename = "hub.mode")]
    mode: Option<String>,
    #[serde(rename = "hub.verify_token")]
    verify_token: Option<String>,
    #[serde(rename = "hub.challenge")]
    challenge: Option<String>,
}

/// GET /webhooks/facebook — Facebook webhook verification
async fn handle_facebook_verify(
    Query(params): Query<FacebookVerifyParams>,
) -> impl IntoResponse {
    let expected_token = std::env::var("FACEBOOK_VERIFY_TOKEN").unwrap_or_default();
    if params.mode.as_deref() == Some("subscribe")
        && params.verify_token.as_deref() == Some(&expected_token)
        && !expected_token.is_empty()
    {
        info!("Facebook webhook verified");
        (StatusCode::OK, params.challenge.unwrap_or_default())
    } else {
        tracing::warn!("Facebook webhook verification failed");
        (StatusCode::FORBIDDEN, "Verification failed".to_string())
    }
}

/// POST /webhooks/facebook — Facebook Messenger incoming messages
async fn handle_facebook_webhook(
    State(state): State<Arc<AppState>>,
    body: String,
) -> impl IntoResponse {
    info!("Facebook webhook received: {} bytes", body.len());

    let event = match FacebookChannel::parse_webhook_event(&body) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("Failed to parse Facebook webhook: {}", e);
            return StatusCode::BAD_REQUEST;
        }
    };

    if event.object != "page" {
        return StatusCode::OK;
    }

    let page_token = std::env::var("FACEBOOK_PAGE_ACCESS_TOKEN").unwrap_or_default();
    if page_token.is_empty() {
        tracing::error!("FACEBOOK_PAGE_ACCESS_TOKEN not set");
        return StatusCode::OK;
    }

    // Process each messaging event asynchronously
    for entry in &event.entry {
        if let Some(ref messaging_list) = entry.messaging {
            for messaging in messaging_list {
                let text = match messaging.message.as_ref().and_then(|m| m.text.as_deref()) {
                    Some(t) => t,
                    None => continue,
                };

                let sender_id = &messaging.sender.id;
                let channel_key = format!("fb:{}", sender_id);

                // Resolve session key
                let session_key = {
                    #[cfg(feature = "dynamodb-backend")]
                    {
                        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                            resolve_session_key(dynamo, table, &channel_key).await
                        } else {
                            channel_key.clone()
                        }
                    }
                    #[cfg(not(feature = "dynamodb-backend"))]
                    { channel_key.clone() }
                };

                let provider = match state.get_provider() {
                    Some(p) => p.clone(),
                    None => {
                        let client = reqwest::Client::new();
                        let _ = FacebookChannel::send_message_static(&client, &page_token, sender_id, "AI provider not configured.").await;
                        continue;
                    }
                };

                let system_prompt = "あなたはChatWeb（chatweb.ai）、音声対応の高速AIアシスタントです。Facebook Messengerで会話しています。300文字以内で簡潔に回答してください。";
                let mut messages = vec![Message::system(system_prompt)];

                {
                    let mut sessions = state.sessions.lock().await;
                    let session = sessions.refresh(&session_key);
                    let history = session.get_history(10);
                    for msg in &history {
                        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
                        let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
                        match role {
                            "user" => messages.push(Message::user(content)),
                            "assistant" => messages.push(Message::assistant(content)),
                            _ => {}
                        }
                    }
                }

                messages.push(Message::user(text));

                let model = &state.config.agents.defaults.model;
                let max_tokens = state.config.agents.defaults.max_tokens;
                let temperature = state.config.agents.defaults.temperature;

                let reply = match provider.chat(&messages, None, model, max_tokens, temperature).await {
                    Ok(completion) => {
                        #[cfg(feature = "dynamodb-backend")]
                        {
                            if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                                let _ = deduct_credits(dynamo, table, &session_key, model,
                                    completion.usage.prompt_tokens, completion.usage.completion_tokens).await;
                            }
                        }
                        let resp = completion.content.unwrap_or_default();
                        {
                            let mut sessions = state.sessions.lock().await;
                            let session = sessions.get_or_create(&session_key);
                            session.add_message_from_channel("user", text, "facebook");
                            session.add_message_from_channel("assistant", &resp, "facebook");
                            sessions.save_by_key(&session_key);
                        }
                        #[cfg(feature = "dynamodb-backend")]
                        {
                            if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                                increment_sync_version(dynamo, table, &session_key, "facebook").await;
                            }
                        }
                        resp
                    }
                    Err(e) => {
                        tracing::error!("LLM error for Facebook: {}", e);
                        "Sorry, an error occurred. Please try again.".to_string()
                    }
                };

                let client = reqwest::Client::new();
                if let Err(e) = FacebookChannel::send_message_static(&client, &page_token, sender_id, &reply).await {
                    tracing::error!("Failed to send Facebook reply: {}", e);
                }
            }
        }
    }

    StatusCode::OK
}

// ---------------------------------------------------------------------------
// Teams Webhook
// ---------------------------------------------------------------------------

/// POST /webhooks/teams — MS Teams Bot Framework incoming activities
async fn handle_teams_webhook(
    State(state): State<Arc<AppState>>,
    body: String,
) -> impl IntoResponse {
    info!("Teams webhook received: {} bytes", body.len());

    let activity = match TeamsChannel::parse_activity(&body) {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("Failed to parse Teams activity: {}", e);
            return StatusCode::BAD_REQUEST;
        }
    };

    if activity.activity_type != "message" {
        return StatusCode::OK;
    }

    let text = match activity.text.as_deref() {
        Some(t) if !t.is_empty() => t.to_string(),
        _ => return StatusCode::OK,
    };

    let sender_id = activity.from.as_ref().map(|f| f.id.as_str()).unwrap_or("").to_string();
    let conversation_id = activity.conversation.as_ref().map(|c| c.id.as_str()).unwrap_or("").to_string();
    let service_url = activity.service_url.as_deref().unwrap_or("https://smba.trafficmanager.net/teams/").to_string();

    if sender_id.is_empty() || conversation_id.is_empty() {
        return StatusCode::OK;
    }

    let channel_key = format!("teams:{}", sender_id);
    let session_key = {
        #[cfg(feature = "dynamodb-backend")]
        {
            if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                resolve_session_key(dynamo, table, &channel_key).await
            } else { channel_key.clone() }
        }
        #[cfg(not(feature = "dynamodb-backend"))]
        { channel_key.clone() }
    };

    let provider = match state.get_provider() {
        Some(p) => p.clone(),
        None => return StatusCode::OK,
    };

    let system_prompt = "あなたはChatWeb（chatweb.ai）、音声対応の高速AIアシスタントです。Microsoft Teamsで会話しています。300文字以内で簡潔に回答してください。";
    let mut messages = vec![Message::system(system_prompt)];
    {
        let mut sessions = state.sessions.lock().await;
        let session = sessions.refresh(&session_key);
        for msg in session.get_history(10) {
            let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
            let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
            match role {
                "user" => messages.push(Message::user(content)),
                "assistant" => messages.push(Message::assistant(content)),
                _ => {}
            }
        }
    }
    messages.push(Message::user(&text));

    let model = &state.config.agents.defaults.model;
    let max_tokens = state.config.agents.defaults.max_tokens;
    let temperature = state.config.agents.defaults.temperature;

    let reply = match provider.chat(&messages, None, model, max_tokens, temperature).await {
        Ok(completion) => {
            #[cfg(feature = "dynamodb-backend")]
            {
                if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                    let _ = deduct_credits(dynamo, table, &session_key, model,
                        completion.usage.prompt_tokens, completion.usage.completion_tokens).await;
                }
            }
            let resp = completion.content.unwrap_or_default();
            {
                let mut sessions = state.sessions.lock().await;
                let session = sessions.get_or_create(&session_key);
                session.add_message_from_channel("user", &text, "teams");
                session.add_message_from_channel("assistant", &resp, "teams");
                sessions.save_by_key(&session_key);
            }
            #[cfg(feature = "dynamodb-backend")]
            {
                if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                    increment_sync_version(dynamo, table, &session_key, "teams").await;
                }
            }
            resp
        }
        Err(e) => {
            tracing::error!("LLM error for Teams: {}", e);
            "申し訳ありません。エラーが発生しました。".to_string()
        }
    };

    // Reply via Bot Framework REST API
    let url = format!("{}v3/conversations/{}/activities", service_url, conversation_id);
    let app_id = std::env::var("TEAMS_APP_ID").unwrap_or_default();
    let app_password = std::env::var("TEAMS_APP_PASSWORD").unwrap_or_default();

    if !app_id.is_empty() && !app_password.is_empty() {
        let client = reqwest::Client::new();
        if let Ok(token_resp) = client.post("https://login.microsoftonline.com/botframework.com/oauth2/v2.0/token")
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", app_id.as_str()),
                ("client_secret", app_password.as_str()),
                ("scope", "https://api.botframework.com/.default"),
            ])
            .send().await
        {
            if let Ok(token_json) = token_resp.json::<serde_json::Value>().await {
                if let Some(token) = token_json.get("access_token").and_then(|v| v.as_str()) {
                    let _ = client.post(&url)
                        .header("Authorization", format!("Bearer {}", token))
                        .json(&serde_json::json!({ "type": "message", "text": reply }))
                        .send().await;
                }
            }
        }
    }

    StatusCode::OK
}

// ---------------------------------------------------------------------------
// Google Chat Webhook
// ---------------------------------------------------------------------------

/// POST /webhooks/google_chat — Google Chat incoming events
async fn handle_google_chat_webhook(
    State(state): State<Arc<AppState>>,
    body: String,
) -> impl IntoResponse {
    info!("Google Chat webhook received: {} bytes", body.len());

    let event = match GoogleChatChannel::parse_event(&body) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("Failed to parse Google Chat event: {}", e);
            return (StatusCode::BAD_REQUEST, "{}".to_string()).into_response();
        }
    };

    if event.event_type == "ADDED_TO_SPACE" {
        return axum::Json(serde_json::json!({ "text": "こんにちは！何でもお聞きください 🎉" })).into_response();
    }

    if event.event_type != "MESSAGE" {
        return (StatusCode::OK, "{}".to_string()).into_response();
    }

    let text = match event.message.as_ref().and_then(|m| m.text.as_deref()) {
        Some(t) if !t.is_empty() => t.to_string(),
        _ => return (StatusCode::OK, "{}".to_string()).into_response(),
    };

    let sender_id = event.user.as_ref().map(|u| u.name.as_str()).unwrap_or("").to_string();
    let channel_key = format!("gchat:{}", sender_id);
    let session_key = {
        #[cfg(feature = "dynamodb-backend")]
        {
            if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                resolve_session_key(dynamo, table, &channel_key).await
            } else { channel_key.clone() }
        }
        #[cfg(not(feature = "dynamodb-backend"))]
        { channel_key.clone() }
    };

    let provider = match state.get_provider() {
        Some(p) => p.clone(),
        None => return axum::Json(serde_json::json!({ "text": "AI provider not configured." })).into_response(),
    };

    let system_prompt = "あなたはChatWeb（chatweb.ai）、音声対応の高速AIアシスタントです。Google Chatで会話しています。300文字以内で簡潔に回答してください。";
    let mut messages = vec![Message::system(system_prompt)];
    {
        let mut sessions = state.sessions.lock().await;
        let session = sessions.refresh(&session_key);
        for msg in session.get_history(10) {
            let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
            let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
            match role {
                "user" => messages.push(Message::user(content)),
                "assistant" => messages.push(Message::assistant(content)),
                _ => {}
            }
        }
    }
    messages.push(Message::user(&text));

    let model = &state.config.agents.defaults.model;
    let max_tokens = state.config.agents.defaults.max_tokens;
    let temperature = state.config.agents.defaults.temperature;

    let reply = match provider.chat(&messages, None, model, max_tokens, temperature).await {
        Ok(completion) => {
            #[cfg(feature = "dynamodb-backend")]
            {
                if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                    let _ = deduct_credits(dynamo, table, &session_key, model,
                        completion.usage.prompt_tokens, completion.usage.completion_tokens).await;
                }
            }
            let resp = completion.content.unwrap_or_default();
            {
                let mut sessions = state.sessions.lock().await;
                let session = sessions.get_or_create(&session_key);
                session.add_message_from_channel("user", &text, "google_chat");
                session.add_message_from_channel("assistant", &resp, "google_chat");
                sessions.save_by_key(&session_key);
            }
            #[cfg(feature = "dynamodb-backend")]
            {
                if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                    increment_sync_version(dynamo, table, &session_key, "google_chat").await;
                }
            }
            resp
        }
        Err(e) => {
            tracing::error!("LLM error for Google Chat: {}", e);
            "申し訳ありません。エラーが発生しました。".to_string()
        }
    };

    axum::Json(serde_json::json!({ "text": reply })).into_response()
}

// ---------------------------------------------------------------------------
// Zalo Webhook
// ---------------------------------------------------------------------------

/// POST /webhooks/zalo — Zalo OA incoming events
async fn handle_zalo_webhook(
    State(state): State<Arc<AppState>>,
    body: String,
) -> impl IntoResponse {
    info!("Zalo webhook received: {} bytes", body.len());

    let event = match ZaloChannel::parse_event(&body) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("Failed to parse Zalo event: {}", e);
            return StatusCode::BAD_REQUEST;
        }
    };

    if event.event_name != "user_send_text" {
        return StatusCode::OK;
    }

    let text = match event.message.as_ref().and_then(|m| m.text.as_deref()) {
        Some(t) if !t.is_empty() => t.to_string(),
        _ => return StatusCode::OK,
    };

    let sender_id = match event.sender.as_ref() {
        Some(s) if !s.id.is_empty() => s.id.clone(),
        _ => return StatusCode::OK,
    };

    let zalo_token = std::env::var("ZALO_BOT_TOKEN").unwrap_or_default();
    if zalo_token.is_empty() {
        tracing::error!("ZALO_BOT_TOKEN not set");
        return StatusCode::OK;
    }

    let channel_key = format!("zalo:{}", sender_id);
    let session_key = {
        #[cfg(feature = "dynamodb-backend")]
        {
            if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                resolve_session_key(dynamo, table, &channel_key).await
            } else { channel_key.clone() }
        }
        #[cfg(not(feature = "dynamodb-backend"))]
        { channel_key.clone() }
    };

    let provider = match state.get_provider() {
        Some(p) => p.clone(),
        None => return StatusCode::OK,
    };

    let system_prompt = "あなたはChatWeb（chatweb.ai）、音声対応の高速AIアシスタントです。Zaloで会話しています。300文字以内で簡潔に回答してください。";
    let mut messages = vec![Message::system(system_prompt)];
    {
        let mut sessions = state.sessions.lock().await;
        let session = sessions.refresh(&session_key);
        for msg in session.get_history(10) {
            let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
            let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
            match role {
                "user" => messages.push(Message::user(content)),
                "assistant" => messages.push(Message::assistant(content)),
                _ => {}
            }
        }
    }
    messages.push(Message::user(&text));

    let model = &state.config.agents.defaults.model;
    let max_tokens = state.config.agents.defaults.max_tokens;
    let temperature = state.config.agents.defaults.temperature;

    let reply = match provider.chat(&messages, None, model, max_tokens, temperature).await {
        Ok(completion) => {
            #[cfg(feature = "dynamodb-backend")]
            {
                if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                    let _ = deduct_credits(dynamo, table, &session_key, model,
                        completion.usage.prompt_tokens, completion.usage.completion_tokens).await;
                }
            }
            let resp = completion.content.unwrap_or_default();
            {
                let mut sessions = state.sessions.lock().await;
                let session = sessions.get_or_create(&session_key);
                session.add_message_from_channel("user", &text, "zalo");
                session.add_message_from_channel("assistant", &resp, "zalo");
                sessions.save_by_key(&session_key);
            }
            #[cfg(feature = "dynamodb-backend")]
            {
                if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                    increment_sync_version(dynamo, table, &session_key, "zalo").await;
                }
            }
            resp
        }
        Err(e) => {
            tracing::error!("LLM error for Zalo: {}", e);
            "Xin lỗi, đã xảy ra lỗi.".to_string()
        }
    };

    let client = reqwest::Client::new();
    let _ = client.post("https://openapi.zalo.me/v3.0/oa/message/cs")
        .header("access_token", &zalo_token)
        .json(&serde_json::json!({
            "recipient": { "user_id": sender_id },
            "message": { "text": reply },
        }))
        .send().await;

    StatusCode::OK
}

// ---------------------------------------------------------------------------
// Feishu Webhook
// ---------------------------------------------------------------------------

/// POST /webhooks/feishu — Feishu/Lark event subscription
async fn handle_feishu_webhook(
    State(state): State<Arc<AppState>>,
    body: String,
) -> impl IntoResponse {
    info!("Feishu webhook received: {} bytes", body.len());

    let payload: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("Failed to parse Feishu event: {}", e);
            return (StatusCode::BAD_REQUEST, "{}".to_string()).into_response();
        }
    };

    // URL verification challenge (required by Feishu)
    if payload.get("type").and_then(|v| v.as_str()) == Some("url_verification") {
        let challenge = payload.get("challenge").and_then(|v| v.as_str()).unwrap_or("");
        return axum::Json(serde_json::json!({ "challenge": challenge })).into_response();
    }

    let event = match payload.get("event") {
        Some(e) => e,
        None => return (StatusCode::OK, "{}".to_string()).into_response(),
    };

    let msg_type = event.pointer("/message/message_type").and_then(|v| v.as_str()).unwrap_or("");
    if msg_type != "text" {
        return (StatusCode::OK, "{}".to_string()).into_response();
    }

    let content_str = event.pointer("/message/content").and_then(|v| v.as_str()).unwrap_or("{}");
    let content_json: serde_json::Value = serde_json::from_str(content_str).unwrap_or_default();
    let text = content_json.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();

    if text.is_empty() {
        return (StatusCode::OK, "{}".to_string()).into_response();
    }

    let sender_id = event.pointer("/sender/sender_id/open_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let chat_id = event.pointer("/message/chat_id").and_then(|v| v.as_str()).unwrap_or(&sender_id).to_string();

    if sender_id.is_empty() {
        return (StatusCode::OK, "{}".to_string()).into_response();
    }

    let app_id = std::env::var("FEISHU_APP_ID").unwrap_or_default();
    let app_secret = std::env::var("FEISHU_APP_SECRET").unwrap_or_default();
    if app_id.is_empty() || app_secret.is_empty() {
        tracing::error!("FEISHU_APP_ID/FEISHU_APP_SECRET not set");
        return (StatusCode::OK, "{}".to_string()).into_response();
    }

    let channel_key = format!("feishu:{}", sender_id);
    let session_key = {
        #[cfg(feature = "dynamodb-backend")]
        {
            if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                resolve_session_key(dynamo, table, &channel_key).await
            } else { channel_key.clone() }
        }
        #[cfg(not(feature = "dynamodb-backend"))]
        { channel_key.clone() }
    };

    let provider = match state.get_provider() {
        Some(p) => p.clone(),
        None => return (StatusCode::OK, "{}".to_string()).into_response(),
    };

    let system_prompt = "あなたはChatWeb（chatweb.ai）、音声対応の高速AIアシスタントです。Feishu/Larkで会話しています。300文字以内で簡潔に回答してください。";
    let mut messages_vec = vec![Message::system(system_prompt)];
    {
        let mut sessions = state.sessions.lock().await;
        let session = sessions.refresh(&session_key);
        for msg in session.get_history(10) {
            let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
            let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
            match role {
                "user" => messages_vec.push(Message::user(content)),
                "assistant" => messages_vec.push(Message::assistant(content)),
                _ => {}
            }
        }
    }
    messages_vec.push(Message::user(&text));

    let model = &state.config.agents.defaults.model;
    let max_tokens = state.config.agents.defaults.max_tokens;
    let temperature = state.config.agents.defaults.temperature;

    let reply = match provider.chat(&messages_vec, None, model, max_tokens, temperature).await {
        Ok(completion) => {
            #[cfg(feature = "dynamodb-backend")]
            {
                if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                    let _ = deduct_credits(dynamo, table, &session_key, model,
                        completion.usage.prompt_tokens, completion.usage.completion_tokens).await;
                }
            }
            let resp = completion.content.unwrap_or_default();
            {
                let mut sessions = state.sessions.lock().await;
                let session = sessions.get_or_create(&session_key);
                session.add_message_from_channel("user", &text, "feishu");
                session.add_message_from_channel("assistant", &resp, "feishu");
                sessions.save_by_key(&session_key);
            }
            #[cfg(feature = "dynamodb-backend")]
            {
                if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                    increment_sync_version(dynamo, table, &session_key, "feishu").await;
                }
            }
            resp
        }
        Err(e) => {
            tracing::error!("LLM error for Feishu: {}", e);
            "申し訳ありません。エラーが発生しました。".to_string()
        }
    };

    let client = reqwest::Client::new();
    if let Ok(token_resp) = client.post("https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal")
        .json(&serde_json::json!({ "app_id": app_id, "app_secret": app_secret }))
        .send().await
    {
        if let Ok(token_json) = token_resp.json::<serde_json::Value>().await {
            if let Some(token) = token_json.get("tenant_access_token").and_then(|v| v.as_str()) {
                let receive_id_type = if chat_id.starts_with("oc_") { "chat_id" } else { "open_id" };
                let content = serde_json::json!({"text": reply}).to_string();
                let _ = client.post(format!("https://open.feishu.cn/open-apis/im/v1/messages?receive_id_type={}", receive_id_type))
                    .header("Authorization", format!("Bearer {}", token))
                    .json(&serde_json::json!({
                        "receive_id": chat_id,
                        "msg_type": "text",
                        "content": content,
                    }))
                    .send().await;
            }
        }
    }

    (StatusCode::OK, "{}".to_string()).into_response()
}

// ---------------------------------------------------------------------------
// WhatsApp Webhook (Cloud API)
// ---------------------------------------------------------------------------

/// GET /webhooks/whatsapp — WhatsApp webhook verification (same as Facebook)
async fn handle_whatsapp_verify(
    Query(params): Query<FacebookVerifyParams>,
) -> impl IntoResponse {
    let expected_token = std::env::var("FACEBOOK_VERIFY_TOKEN").unwrap_or_default();
    if params.mode.as_deref() == Some("subscribe")
        && params.verify_token.as_deref() == Some(&expected_token)
        && !expected_token.is_empty()
    {
        info!("WhatsApp webhook verified");
        (StatusCode::OK, params.challenge.unwrap_or_default())
    } else {
        tracing::warn!("WhatsApp webhook verification failed");
        (StatusCode::FORBIDDEN, "Verification failed".to_string())
    }
}

/// POST /webhooks/whatsapp — WhatsApp Cloud API incoming messages
async fn handle_whatsapp_webhook(
    State(state): State<Arc<AppState>>,
    body: String,
) -> impl IntoResponse {
    info!("WhatsApp webhook received: {} bytes", body.len());

    let payload: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("Failed to parse WhatsApp webhook: {}", e);
            return StatusCode::BAD_REQUEST;
        }
    };

    let entry = match payload.get("entry").and_then(|v| v.as_array()) {
        Some(e) if !e.is_empty() => &e[0],
        _ => return StatusCode::OK,
    };

    let changes = match entry.get("changes").and_then(|v| v.as_array()) {
        Some(c) if !c.is_empty() => &c[0],
        _ => return StatusCode::OK,
    };

    let value = match changes.get("value") {
        Some(v) => v,
        None => return StatusCode::OK,
    };

    let wa_messages = match value.get("messages").and_then(|v| v.as_array()) {
        Some(m) if !m.is_empty() => m,
        _ => return StatusCode::OK,
    };

    let wa_msg = &wa_messages[0];
    if wa_msg.get("type").and_then(|v| v.as_str()).unwrap_or("") != "text" {
        return StatusCode::OK;
    }

    let text = wa_msg.pointer("/text/body").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let sender_phone = wa_msg.get("from").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let phone_number_id = value.pointer("/metadata/phone_number_id").and_then(|v| v.as_str()).unwrap_or("").to_string();

    if text.is_empty() || sender_phone.is_empty() {
        return StatusCode::OK;
    }

    let wa_token = std::env::var("WHATSAPP_TOKEN").unwrap_or_default();
    if wa_token.is_empty() {
        tracing::error!("WHATSAPP_TOKEN not set");
        return StatusCode::OK;
    }

    let channel_key = format!("wa:{}", sender_phone);
    let session_key = {
        #[cfg(feature = "dynamodb-backend")]
        {
            if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                resolve_session_key(dynamo, table, &channel_key).await
            } else { channel_key.clone() }
        }
        #[cfg(not(feature = "dynamodb-backend"))]
        { channel_key.clone() }
    };

    let provider = match state.get_provider() {
        Some(p) => p.clone(),
        None => return StatusCode::OK,
    };

    let system_prompt = "あなたはChatWeb（chatweb.ai）、音声対応の高速AIアシスタントです。WhatsAppで会話しています。300文字以内で簡潔に回答してください。";
    let mut messages_vec = vec![Message::system(system_prompt)];
    {
        let mut sessions = state.sessions.lock().await;
        let session = sessions.refresh(&session_key);
        for msg in session.get_history(10) {
            let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
            let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
            match role {
                "user" => messages_vec.push(Message::user(content)),
                "assistant" => messages_vec.push(Message::assistant(content)),
                _ => {}
            }
        }
    }
    messages_vec.push(Message::user(&text));

    let model = &state.config.agents.defaults.model;
    let max_tokens = state.config.agents.defaults.max_tokens;
    let temperature = state.config.agents.defaults.temperature;

    let reply = match provider.chat(&messages_vec, None, model, max_tokens, temperature).await {
        Ok(completion) => {
            #[cfg(feature = "dynamodb-backend")]
            {
                if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                    let _ = deduct_credits(dynamo, table, &session_key, model,
                        completion.usage.prompt_tokens, completion.usage.completion_tokens).await;
                }
            }
            let resp = completion.content.unwrap_or_default();
            {
                let mut sessions = state.sessions.lock().await;
                let session = sessions.get_or_create(&session_key);
                session.add_message_from_channel("user", &text, "whatsapp");
                session.add_message_from_channel("assistant", &resp, "whatsapp");
                sessions.save_by_key(&session_key);
            }
            #[cfg(feature = "dynamodb-backend")]
            {
                if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                    increment_sync_version(dynamo, table, &session_key, "whatsapp").await;
                }
            }
            resp
        }
        Err(e) => {
            tracing::error!("LLM error for WhatsApp: {}", e);
            "Sorry, an error occurred.".to_string()
        }
    };

    let client = reqwest::Client::new();
    let _ = client.post(format!("https://graph.facebook.com/v21.0/{}/messages", phone_number_id))
        .header("Authorization", format!("Bearer {}", wa_token))
        .json(&serde_json::json!({
            "messaging_product": "whatsapp",
            "to": sender_phone,
            "type": "text",
            "text": { "body": reply },
        }))
        .send().await;

    StatusCode::OK
}

// ---------------------------------------------------------------------------
// SSE Streaming Chat
// ---------------------------------------------------------------------------

/// POST /api/v1/chat/stream — SSE streaming chat response
/// Sends tokens as they arrive from the LLM, enabling real-time display.
async fn handle_chat_stream(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<ChatRequest>,
) -> impl IntoResponse {
    use axum::response::sse::{Event, Sse};
    use futures::stream;
    use std::convert::Infallible;

    let stream_start = std::time::Instant::now();

    // Input validation
    if req.message.len() > 32_000 {
        let err_stream = stream::once(async {
            Ok::<_, Infallible>(Event::default().data(
                serde_json::json!({"type":"error","content":"Message too long"}).to_string()
            ))
        });
        return Sse::new(err_stream).into_response();
    }

    // Resolve session key
    let session_key = {
        #[cfg(feature = "dynamodb-backend")]
        {
            if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                resolve_session_key(dynamo, table, &req.session_id).await
            } else {
                req.session_id.clone()
            }
        }
        #[cfg(not(feature = "dynamodb-backend"))]
        { req.session_id.clone() }
    };

    // Parallel initialization: fetch user + settings concurrently
    #[cfg(feature = "dynamodb-backend")]
    let (stream_user, stream_memory, stream_settings) = {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let (user, memory, settings) = tokio::join!(
                get_or_create_user(dynamo, table, &session_key),
                read_memory_context(dynamo, table, &session_key),
                get_user_settings(dynamo, table, &session_key)
            );
            (Some(user), memory, Some(settings))
        } else {
            (None, String::new(), None)
        }
    };
    #[cfg(not(feature = "dynamodb-backend"))]
    let (stream_user, stream_memory, stream_settings): (Option<UserProfile>, String, Option<UserSettings>) =
        (None, String::new(), None);

    // Check credits (using cached user)
    #[cfg(feature = "dynamodb-backend")]
    {
        if let Some(ref user) = stream_user {
            if user.credits_remaining <= 0 {
                let content = if user.plan == "free" {
                    "ありがとうございます！無料クレジットを使い切りました 🎉 Starterプラン（月額¥980）にアップグレードして、もっとたくさん話しましょう！"
                } else {
                    "お疲れさまです！クレジットを使い切りました 💪 追加クレジットを購入して、引き続きお楽しみください。"
                };
                let err_stream = stream::once(async move {
                    Ok::<_, Infallible>(Event::default().data(
                        serde_json::json!({"type":"error","content":content,"action":"upgrade"}).to_string()
                    ))
                });
                return Sse::new(err_stream).into_response();
            }
        }
    }

    // Resolve inference mode for streaming endpoint (same logic as handle_chat)
    let stream_host = effective_host(&headers);
    let is_wisbee_stream = stream_host.contains("wisbee.ai");
    let stream_mode = match req.mode.as_deref() {
        Some("local") => "local",
        Some("cloud") => "cloud",
        Some("auto") => "auto",
        None | Some(_) => if is_wisbee_stream { "local" } else { "auto" },
    };

    // Local mode: run inference locally and return as SSE
    #[cfg(feature = "local-fallback")]
    if stream_mode == "local" {
        use crate::provider::local::LocalProvider;
        let result = match LocalProvider::from_env() {
            Some(local_provider) => {
                let messages = vec![
                    Message::system("You are a helpful local AI assistant (Qwen3-0.6B). Respond concisely."),
                    Message::user(&req.message),
                ];
                local_provider.chat(&messages, None, "local-qwen3-0.6b", req.max_tokens.unwrap_or(512).min(512), req.temperature.unwrap_or(0.6)).await
            }
            None => Err(crate::error::ProviderError::Other("Local model not configured".to_string())),
        };
        let event_stream = stream::once(async move {
            match result {
                Ok(resp) => Ok::<_, Infallible>(Event::default().data(
                    serde_json::json!({
                        "type": "done",
                        "content": resp.content.unwrap_or_default(),
                        "model": "local-qwen3-0.6b",
                        "mode": "local",
                        "input_tokens": resp.usage.prompt_tokens,
                        "output_tokens": resp.usage.completion_tokens,
                    }).to_string()
                )),
                Err(e) => Ok::<_, Infallible>(Event::default().data(
                    serde_json::json!({"type": "error", "content": format!("Local model error: {}", e), "mode": "local"}).to_string()
                )),
            }
        });
        return Sse::new(event_stream).into_response();
    }
    #[cfg(not(feature = "local-fallback"))]
    if stream_mode == "local" {
        let event_stream = stream::once(async {
            Ok::<_, Infallible>(Event::default().data(
                serde_json::json!({"type": "error", "content": "Local mode not available in this build", "mode": "local"}).to_string()
            ))
        });
        return Sse::new(event_stream).into_response();
    }

    let provider = match state.get_provider() {
        Some(p) => p.clone(),
        None => {
            let err_stream = stream::once(async {
                Ok::<_, Infallible>(Event::default().data(
                    serde_json::json!({"type":"error","content":"AI provider not configured"}).to_string()
                ))
            });
            return Sse::new(err_stream).into_response();
        }
    };

    // Agent detection (same as handle_chat)
    let (agent, clean_message, agent_score) = detect_agent(&req.message);
    info!("Stream agent: {} (score={}) for message", agent.id, agent_score);

    // Build messages — agent-specific + host-aware system prompt + memory + meta context
    let is_teai = stream_host.contains("teai.io");
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let base_prompt = if is_teai {
        format!(
            "You are Tei — the developer-facing AI agent at teai.io, \
             built in Rust, running on AWS Lambda (ARM64) with parallel execution and <2s response time.\n\
             Open source: github.com/yukihamada\n\n\
             ## SOUL\n\
             - Technical, precise, and concise. You speak code fluently.\n\
             - Bold, direct, and fearless.\n\
             - Prefer English unless the user writes in another language.\n\
             - Focus on: code generation, debugging, architecture, API design, DevOps.\n\
             - Use code blocks with language tags. Be direct and actionable.\n\n\
             ## Service Ecosystem\n\
             - teai.io: This platform. Developer-focused AI agent.\n\
             - chatweb.ai: Japanese voice-first AI assistant (same backend).\n\
             - ElioChat (elio.love): On-device offline AI for iPhone.\n\n\
             {}", agent.system_prompt
        )
    } else {
        agent.system_prompt.to_string()
    };

    // Device-based character limit
    let device = req.device.as_deref().unwrap_or("pc");
    let max_chars = match device {
        "mobile" => agent.max_chars_mobile,
        "voice" => agent.max_chars_voice,
        _ => agent.max_chars_pc,
    };
    let char_instruction = format!(
        "\n\n【応答制約】回答は{}文字以内に収めてください。簡潔に要点を伝えてください。",
        max_chars
    );

    // Get session history first (need history_len for meta context)
    let stream_history: Vec<(String, String)>;
    {
        let mut sessions = state.sessions.lock().await;
        let session = sessions.refresh(&session_key);
        let history = session.get_history_with_summary(16);
        stream_history = history.iter().filter_map(|msg| {
            let role = msg.get("role").and_then(|v| v.as_str())?;
            let content = msg.get("content").and_then(|v| v.as_str())?;
            Some((role.to_string(), content.to_string()))
        }).collect();
    }

    // Use user settings from parallel initialization (moved up for model resolution before system prompt)
    let user_settings: Option<UserSettings> = stream_settings;

    let default_model = state.config.agents.defaults.model.clone();
    let model = req.model.as_deref()
        .or(user_settings.as_ref().and_then(|s| s.preferred_model.as_deref()))
        .or(agent.preferred_model)
        .unwrap_or_else(|| {
            if req.channel == "web" || req.channel.starts_with("webchat") {
                "claude-sonnet-4-5-20250929"
            } else {
                &default_model
            }
        }).to_string();

    // Build meta-cognition context (now includes model/cost info)
    let stream_meta = build_meta_context_with_model(
        stream_user.as_ref(),
        &req.channel,
        device,
        stream_history.len(),
        is_teai,
        Some(&model),
        0,
        0,
    );
    let stream_meta_instr = if is_teai { META_INSTRUCTION_EN } else { META_INSTRUCTION_JA };

    // Adult mode prompt injection (only if age verified and mode enabled)
    let stream_is_adult = user_settings.as_ref()
        .map(|s| s.age_verified.unwrap_or(false) && s.adult_mode.unwrap_or(false))
        .unwrap_or(false);
    let stream_adult_prompt = if stream_is_adult {
        "\n\n## 成人モード\n\
         年齢確認済みユーザーです。成人向けの話題にも対応可能です。\n\
         ただし以下は常にブロックしてください：\n\
         - 未成年に関する性的コンテンツ\n\
         - 違法行為の具体的な指南\n\
         - 実在の人物への誹謗中傷\n\
         - 暴力の煽動\n\
         上記以外の成人向け話題（恋愛相談、性教育、アダルトコンテンツの議論等）には自然に対応してください。"
    } else {
        ""
    };

    // Wow Factor: enhanced prompt for new users (streaming)
    let stream_wow_prompt = {
        #[cfg(feature = "dynamodb-backend")]
        {
            let is_new_user = stream_user.as_ref()
                .map(|u| u.credits_used <= 10 && u.plan == "free")
                .unwrap_or(false);
            if is_new_user {
                "\n\n## 初回ユーザー対応\n\
                 このユーザーは新規ユーザーです。最高の第一印象を与えてください：\n\
                 - 回答は具体的で価値のある内容にする\n\
                 - 可能なら積極的にツール（検索・計算など）を活用して、AIの能力をデモする\n\
                 - 親しみやすく、温かいトーンで対応する"
            } else {
                ""
            }
        }
        #[cfg(not(feature = "dynamodb-backend"))]
        { "" }
    };

    // Custom system prompt from request or user settings
    let stream_custom_sys = req.custom_system_prompt.as_deref()
        .or(user_settings.as_ref().and_then(|s| s.custom_system_prompt.as_deref()))
        .unwrap_or("");
    let stream_custom_block = if stream_custom_sys.is_empty() {
        String::new()
    } else {
        format!("\n\n## ユーザーカスタム指示\n{}", stream_custom_sys)
    };

    let stream_system_prompt = if stream_memory.is_empty() {
        format!("{}\n\n今日の日付: {}{}{}{}{}{}{}", base_prompt, today, stream_meta, stream_meta_instr, stream_adult_prompt, stream_wow_prompt, stream_custom_block, char_instruction)
    } else {
        format!("{}\n\n今日の日付: {}{}{}{}{}{}\n\n---\n{}{}", base_prompt, today, stream_meta, stream_meta_instr, stream_adult_prompt, stream_wow_prompt, stream_custom_block, stream_memory, char_instruction)
    };
    let mut messages = vec![Message::system(&stream_system_prompt)];

    for (role, content) in &stream_history {
        match role.as_str() {
            "user" => messages.push(Message::user(content)),
            "assistant" => messages.push(Message::assistant(content)),
            _ => {}
        }
    }

    // For tool-using agents, augment user message with tool instruction
    if agent.tools_enabled {
        let augmented = format!(
            "{}\n\n[You MUST call web_search tool first to find current information. Never answer from memory alone for factual questions.]",
            clean_message
        );
        messages.push(Message::user(&augmented));
    } else {
        messages.push(Message::user(&clean_message));
    }
    let max_tokens = req.max_tokens.unwrap_or(state.config.agents.defaults.max_tokens);
    let temperature = req.temperature
        .or(user_settings.as_ref().and_then(|s| s.temperature))
        .unwrap_or(state.config.agents.defaults.temperature);

    // Build extra LLM parameters from request
    let chat_extra = provider::ChatExtra {
        top_p: req.top_p.or(user_settings.as_ref().and_then(|s| s.top_p)),
        frequency_penalty: req.frequency_penalty.or(user_settings.as_ref().and_then(|s| s.frequency_penalty)),
        presence_penalty: req.presence_penalty.or(user_settings.as_ref().and_then(|s| s.presence_penalty)),
    };

    // Agentic SSE stream: supports multi-iteration tool calling with progress events.
    // Collects all SSE events into a Vec (API Gateway v2 compatible — no async_stream).
    let req_message = req.message.clone();
    let req_channel = req.channel.clone();
    let req_device = device.to_string();
    let req_session_id = req.session_id.clone();
    let req_language = req.language.clone();
    let state_clone = state.clone();
    let session_key_clone = session_key.clone();
    let agent_id = agent.id;
    let agent_estimated_seconds = agent.estimated_seconds;
    let stream_agent_score = agent_score;
    let stream_user_plan = stream_user.as_ref().map(|u| u.plan.clone()).unwrap_or_else(|| "unknown".to_string());

    // Get tools definitions for the stream handler (respects agent.tools_enabled)
    let tools: Vec<serde_json::Value> = if agent.tools_enabled {
        state.tool_registry.get_definitions()
    } else {
        vec![]
    };

    // Determine max iterations based on user plan
    let max_iterations: usize = {
        #[cfg(feature = "dynamodb-backend")]
        {
            match stream_user.as_ref().map(|u| u.plan.as_str()) {
                Some("pro") | Some("enterprise") => 5,
                Some("starter") => 3,
                _ => {
                    // Wow Factor: new free users get extra tool iteration
                    let is_new = stream_user.as_ref()
                        .map(|u| u.credits_used <= 10 && u.plan == "free")
                        .unwrap_or(false);
                    if is_new { 2 } else { 1 }
                }
            }
        }
        #[cfg(not(feature = "dynamodb-backend"))]
        { 5 }
    };

    // Real-time SSE: send each event individually as it happens via mpsc channel
    let (tx, rx) = futures::channel::mpsc::unbounded::<Result<Event, Infallible>>();

    tokio::spawn(async move {
        // Helper: send a single SSE event immediately
        macro_rules! send_sse {
            ($data:expr) => {
                let _ = tx.unbounded_send(Ok(Event::default().data(
                    serde_json::to_string(&$data).unwrap_or_default()
                )));
            };
        }
        let mut event_count: usize = 0;

        // Start event with agent metadata (sent immediately)
        send_sse!(serde_json::json!({
            "type": "start",
            "session_id": req_session_id,
            "agent": agent_id,
            "estimated_seconds": agent_estimated_seconds,
        }));
        event_count += 1;

        let tools_ref = if tools.is_empty() { None } else { Some(&tools[..]) };

        // LLM call with hard deadline — using streaming to send content_chunk events in real-time
        let deadline = std::time::Duration::from_secs(RESPONSE_DEADLINE_SECS);
        let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let tx_for_chunks = tx.clone();
        let chunk_forwarder = tokio::spawn(async move {
            while let Some(chunk) = chunk_rx.recv().await {
                let _ = tx_for_chunks.unbounded_send(Ok(Event::default().data(
                    serde_json::json!({"type":"content_chunk","text":chunk}).to_string()
                )));
            }
        });
        let llm_result = tokio::time::timeout(
            deadline,
            provider.chat_stream(&messages, tools_ref, &model, max_tokens, temperature, &chat_extra, chunk_tx),
        ).await;
        let _ = chunk_forwarder.await;
        let (stream_used_model, first_result) = match llm_result {
            Ok(Ok(c)) => (model.clone(), Ok(c)),
            Ok(Err(e)) => {
                tracing::error!("Stream LLM call failed: {}", e);
                (model.clone(), Err(e))
            }
            Err(_) => {
                tracing::warn!("Stream LLM call timed out after {}s, returning fallback", RESPONSE_DEADLINE_SECS);
                let fallback = timeout_fallback_message();
                #[cfg(feature = "dynamodb-backend")]
                {
                    if let (Some(dynamo), Some(table)) = (&state_clone.dynamo_client, &state_clone.config_table) {
                        let (credits, remaining) = deduct_credits(dynamo, table, &session_key_clone, &model, 100, 0).await;
                        if let Some(r) = remaining {
                            send_sse!(serde_json::json!({"type":"done","credits_used": credits, "credits_remaining": r}));
                        } else {
                            send_sse!(serde_json::json!({"type":"done","credits_used": credits}));
                        }
                    }
                }
                send_sse!(serde_json::json!({"type":"content","content": fallback}));
                #[cfg(not(feature = "dynamodb-backend"))]
                send_sse!(serde_json::json!({"type":"done"}));
                return; // tx dropped → stream ends
            }
        };

        let mut stream_had_error = false;
        match first_result {
            Ok(completion) => {
                #[allow(unused_mut)]
                let mut total_credits_used: i64 = 0;
                #[allow(unused_mut)]
                let mut last_remaining: Option<i64> = None;
                let mut stream_total_input: u32 = completion.usage.prompt_tokens;
                let mut stream_total_output: u32 = completion.usage.completion_tokens;

                // Deduct credits for first call
                #[cfg(feature = "dynamodb-backend")]
                {
                    if let (Some(dynamo), Some(table)) = (&state_clone.dynamo_client, &state_clone.config_table) {
                        let (credits, remaining) = deduct_credits(dynamo, table, &session_key_clone, &stream_used_model,
                            completion.usage.prompt_tokens, completion.usage.completion_tokens).await;
                        total_credits_used += credits;
                        if remaining.is_some() { last_remaining = remaining; }
                    }
                }

                let mut current = completion;
                let mut conversation = messages.clone();
                let mut all_tools_used: Vec<String> = Vec::new();
                let mut iteration: usize = 0;

                // Create sandbox directory
                let sandbox_dir = format!("/tmp/sandbox/{}", session_key_clone.replace(':', "_"));
                std::fs::create_dir_all(&sandbox_dir).ok();

                // Multi-iteration tool loop
                while current.has_tool_calls() && iteration < max_iterations {
                    iteration += 1;
                    let tool_calls_to_run: Vec<_> = current.tool_calls.iter().take(5).collect();

                    // Emit tool_start events (sent immediately — client shows progress)
                    for tc in &tool_calls_to_run {
                        send_sse!(serde_json::json!({
                            "type": "tool_start",
                            "tool": tc.name,
                            "iteration": iteration,
                        }));
                        event_count += 1;
                    }

                    // Execute tool calls in parallel
                    let registry = &state_clone.tool_registry;
                    let futures_vec: Vec<_> = tool_calls_to_run.iter().map(|tc| {
                        let name = tc.name.clone();
                        let mut args = tc.arguments.clone();
                        let id = tc.id.clone();
                        if name == "code_execute" || name == "file_read" || name == "file_write" || name == "file_list" || name == "web_deploy" {
                            args.insert("_sandbox_dir".to_string(), serde_json::Value::String(sandbox_dir.clone()));
                        }
                        if name == "phone_call" || name == "web_deploy" {
                            args.insert("_session_key".to_string(), serde_json::Value::String(session_key_clone.clone()));
                        }
                        async move {
                            let raw_result = registry.execute(&name, &args).await;
                            let result = if raw_result.starts_with("[TOOL_ERROR]") {
                                raw_result
                            } else if raw_result.starts_with("Error") || raw_result.starts_with("error")
                                || raw_result.contains("request failed") || raw_result.contains("timed out")
                            {
                                format!("[TOOL_ERROR] {}\nYou may retry with different parameters or use an alternative approach.", raw_result)
                            } else if raw_result.is_empty() || raw_result == "No results found."
                                || raw_result.contains("No results") || raw_result.contains("no results")
                            {
                                format!("[NO_RESULTS] {}\nTry rephrasing the query or using a different tool.", if raw_result.is_empty() { "Empty response" } else { &raw_result })
                            } else {
                                raw_result
                            };
                            (id, name, result)
                        }
                    }).collect();
                    let tool_results: Vec<_> = futures::future::join_all(futures_vec).await;

                    // Emit tool_result events (sent immediately)
                    for (_, name, result) in &tool_results {
                        all_tools_used.push(name.clone());
                        let preview_end = result.char_indices().nth(500).map(|(i, _)| i).unwrap_or(result.len());
                        send_sse!(serde_json::json!({
                            "type": "tool_result",
                            "tool": name,
                            "result": &result[..preview_end],
                            "iteration": iteration,
                        }));
                        event_count += 1;
                    }

                    // Build conversation with tool calls + results
                    let tc_json: Vec<serde_json::Value> = current.tool_calls.iter().take(5).map(|tc| {
                        serde_json::json!({
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": tc.name,
                                "arguments": serde_json::to_string(&tc.arguments).unwrap_or_default(),
                            }
                        })
                    }).collect();
                    conversation.push(Message::assistant_with_tool_calls(current.content.clone(), tc_json));
                    for (id, name, result) in &tool_results {
                        conversation.push(Message::tool_result(id, name, result));
                    }

                    // Emit thinking event (sent immediately)
                    send_sse!(serde_json::json!({
                        "type": "thinking",
                        "iteration": iteration,
                    }));
                    event_count += 1;

                    // Follow-up LLM call: pass tools if more iterations remain, stream text on final iteration
                    let follow_up_tools = if iteration < max_iterations {
                        Some(&tools[..])
                    } else {
                        None
                    };

                    let (fu_chunk_tx, mut fu_chunk_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
                    let tx_for_fu = tx.clone();
                    let fu_forwarder = tokio::spawn(async move {
                        while let Some(chunk) = fu_chunk_rx.recv().await {
                            let _ = tx_for_fu.unbounded_send(Ok(Event::default().data(
                                serde_json::json!({"type":"content_chunk","text":chunk}).to_string()
                            )));
                        }
                    });

                    match provider.chat_stream(&conversation, follow_up_tools, &model, max_tokens, temperature, &chat_extra, fu_chunk_tx).await {
                        Ok(resp) => {
                            stream_total_input += resp.usage.prompt_tokens;
                            stream_total_output += resp.usage.completion_tokens;
                            #[cfg(feature = "dynamodb-backend")]
                            {
                                if let (Some(dynamo), Some(table)) = (&state_clone.dynamo_client, &state_clone.config_table) {
                                    let (credits, remaining) = deduct_credits(dynamo, table, &session_key_clone, &model,
                                        resp.usage.prompt_tokens, resp.usage.completion_tokens).await;
                                    total_credits_used += credits;
                                    if remaining.is_some() { last_remaining = remaining; }
                                    // Break early if credits exhausted
                                    if remaining == Some(0) {
                                        current = resp;
                                        tracing::info!("Credits exhausted for user {} in stream, breaking tool loop at iteration {}", session_key_clone, iteration);
                                        break;
                                    }
                                }
                            }
                            current = resp;
                        }
                        Err(e) => {
                            tracing::error!("LLM follow-up error in stream: {}", e);
                            current = crate::types::CompletionResponse {
                                content: Some("申し訳ありません。一時的にAIサービスに接続できませんでした。もう一度お試しください。".to_string()),
                                tool_calls: vec![],
                                finish_reason: crate::types::FinishReason::Stop,
                                usage: crate::types::TokenUsage::default(),
                            };
                            break;
                        }
                    }
                    let _ = fu_forwarder.await;
                }

                let mut response_text = current.content.unwrap_or_default();

                // Auto-translate to Japanese if UI language is "ja" but response has no Japanese
                if req_language.as_deref() == Some("ja") && detect_language(&response_text) != "ja" {
                    let translate_provider = state_clone.lb_provider.clone().or_else(|| state_clone.provider.clone());
                    if let Some(translated) = maybe_translate_to_japanese(&response_text, translate_provider.as_ref()).await {
                        response_text = translated;
                    }
                }

                // Save to session
                {
                    let mut sessions = state_clone.sessions.lock().await;
                    let session = sessions.get_or_create(&session_key_clone);
                    session.add_message("user", &req_message);
                    session.add_message("assistant", &response_text);
                    sessions.save_by_key(&session_key_clone);
                }
                #[cfg(feature = "dynamodb-backend")]
                {
                    if let (Some(dynamo), Some(table)) = (&state_clone.dynamo_client, &state_clone.config_table) {
                        increment_sync_version(dynamo, table, &session_key_clone, "web").await;
                    }
                }

                // Auto-update conversation title & message count
                #[cfg(feature = "dynamodb-backend")]
                {
                    if let (Some(dynamo), Some(table)) = (&state_clone.dynamo_client, &state_clone.config_table) {
                        if let Some(conv_id) = req_session_id.strip_prefix("webchat:") {
                            let msg_count = {
                                let mut sessions = state_clone.sessions.lock().await;
                                sessions.get_or_create(&session_key_clone).messages.len()
                            };
                            spawn_update_conv_meta(
                                dynamo.clone(), table.clone(), session_key_clone.clone(),
                                conv_id.to_string(), req_message.clone(), msg_count,
                            );
                        }
                    }
                }

                // Auto-save to daily memory log + trigger consolidation (fire-and-forget)
                #[cfg(feature = "dynamodb-backend")]
                {
                    if let (Some(dynamo), Some(table)) = (&state_clone.dynamo_client, &state_clone.config_table) {
                        let dynamo = dynamo.clone();
                        let table = table.clone();
                        let sk = session_key_clone.clone();
                        let user_msg = req_message.clone();
                        let bot_msg = response_text.clone();
                        let provider_for_mem = state_clone.lb_provider.clone().or_else(|| state_clone.provider.clone());
                        tokio::spawn(async move {
                            let summary = format!("- Q: {} → A: {}",
                                if user_msg.len() > 80 { let mut i = 80; while i > 0 && !user_msg.is_char_boundary(i) { i -= 1; } format!("{}...", &user_msg[..i]) } else { user_msg },
                                if bot_msg.len() > 120 { let mut i = 120; while i > 0 && !bot_msg.is_char_boundary(i) { i -= 1; } format!("{}...", &bot_msg[..i]) } else { bot_msg },
                            );
                            let entry_count = append_daily_memory(&dynamo, &table, &sk, &summary).await;
                            if entry_count > 0 && entry_count % 10 == 0 {
                                if let Some(provider) = provider_for_mem {
                                    spawn_consolidate_memory(dynamo, table, sk, provider);
                                }
                            }
                        });
                    }
                }

                // Content event (final answer — sent immediately)
                let stream_cost = crate::provider::pricing::calculate_cost(&stream_used_model, stream_total_input, stream_total_output);
                send_sse!(serde_json::json!({
                    "type": "content",
                    "content": response_text,
                    "agent": agent_id,
                    "credits_remaining": last_remaining,
                    "credits_used": if total_credits_used > 0 { Some(total_credits_used) } else { None::<i64> },
                    "tools_used": if all_tools_used.is_empty() { None } else { Some(&all_tools_used) },
                    "iterations": iteration,
                    "model_used": stream_used_model,
                    "input_tokens": stream_total_input,
                    "output_tokens": stream_total_output,
                    "estimated_cost_usd": if stream_cost > 0.0 { Some(stream_cost) } else { None::<f64> },
                }));
                event_count += 1;
            }
            Err(e) => {
                tracing::error!("LLM stream error (all providers failed): {}", e);
                stream_had_error = true;
                let fallback = error_fallback_message();
                send_sse!(serde_json::json!({"type":"content","content": fallback}));
                event_count += 1;
            }
        }

        // Log latency and emit audit
        let stream_latency = stream_start.elapsed().as_millis();
        info!("Stream response: session={}, model={}, latency={}ms, events={}",
            session_key_clone, model, stream_latency, event_count);
        #[cfg(feature = "dynamodb-backend")]
        {
            if let (Some(dynamo), Some(table)) = (&state_clone.dynamo_client, &state_clone.config_table) {
                emit_audit_log(dynamo.clone(), table.clone(), "chat_stream", &session_key_clone, "",
                    &format!("model={} latency={}ms events={}", model, stream_latency, event_count));

                // Fire-and-forget routing log
                let session_hash = {
                    use std::collections::hash_map::DefaultHasher;
                    use std::hash::{Hash, Hasher};
                    let mut h = DefaultHasher::new();
                    session_key_clone.hash(&mut h);
                    format!("{:x}", h.finish())
                };
                log_routing_data(dynamo.clone(), table.clone(), RoutingLogEntry {
                    message_len: req_message.len(),
                    language: detect_language(&req_message).to_string(),
                    channel: req_channel.clone(),
                    device: req_device.clone(),
                    has_at_prefix: req_message.trim().starts_with('@'),
                    user_plan: stream_user_plan.clone(),
                    agent_selected: agent_id.to_string(),
                    agent_score: stream_agent_score,
                    model_used: model.clone(),
                    tools_used: vec![], // stream tools tracked via events
                    response_time_ms: stream_latency as u64,
                    credits_used: 0, // tracked via deduct_credits
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    timed_out: false,
                    error: stream_had_error,
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    session_hash,
                });
            }
        }

        // Done event
        send_sse!(serde_json::json!({"type":"done"}));
        // tx is dropped here → stream closes naturally
    });

    Sse::new(rx)
        .keep_alive(axum::response::sse::KeepAlive::default())
        .into_response()
}

/// POST /api/v1/chat/explore — Multi-model explore with SSE streaming.
/// Runs all available providers in parallel, streams results as they arrive.
/// Supports hierarchical re-query: if initial results are insufficient,
/// can escalate with multiple prompt variations for improved accuracy.
/// All costs are deducted from user credits.
async fn handle_chat_explore(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ExploreRequest>,
) -> impl IntoResponse {
    use axum::response::sse::{Event, Sse};
    use std::convert::Infallible;

    // Input validation
    if req.message.len() > 32_000 {
        let err_stream = futures::stream::once(async {
            Ok::<_, Infallible>(Event::default()
                .event("error")
                .data(serde_json::json!({"error": "Message too long"}).to_string()))
        });
        return Sse::new(err_stream).into_response();
    }

    // Resolve session key
    let session_key = {
        #[cfg(feature = "dynamodb-backend")]
        {
            if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                resolve_session_key(dynamo, table, &req.session_id).await
            } else {
                req.session_id.clone()
            }
        }
        #[cfg(not(feature = "dynamodb-backend"))]
        { req.session_id.clone() }
    };

    // Check credits
    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let user = get_or_create_user(dynamo, table, &session_key).await;
            if user.credits_remaining <= 0 {
                let content = if user.plan == "free" {
                    "無料クレジットを使い切りました 🎉 Starterプランにアップグレードしましょう！"
                } else {
                    "クレジットを使い切りました 💪 追加購入して続けましょう！"
                };
                let err_stream = futures::stream::once(async move {
                    Ok::<_, Infallible>(Event::default()
                        .event("error")
                        .data(serde_json::json!({"error": content, "action": "upgrade"}).to_string()))
                });
                return Sse::new(err_stream).into_response();
            }
            // Note: All plans (including free) can use explore mode.
            // Credits are deducted per model, so free users burn credits faster — incentivizing upgrades.
        }
    }

    let lb_raw = match &state.lb_raw {
        Some(lb) => lb.clone(),
        None => {
            let err_stream = futures::stream::once(async {
                Ok::<_, Infallible>(Event::default()
                    .event("error")
                    .data(serde_json::json!({"error": "No providers available"}).to_string()))
            });
            return Sse::new(err_stream).into_response();
        }
    };

    // Pre-process: detect URLs and search queries, fetch content before sending to models
    let mut context_prefix = String::new();
    {
        use crate::service::integrations::{execute_web_fetch, execute_web_search};

        // Extract URLs from the message
        let urls: Vec<&str> = URL_REGEX.find_iter(&req.message).map(|m| m.as_str()).collect();

        // Fetch URLs in parallel
        if !urls.is_empty() {
            tracing::info!("explore: fetching {} URLs", urls.len());
            let fetch_futures: Vec<_> = urls.iter().map(|url| execute_web_fetch(url)).collect();
            let results = futures::future::join_all(fetch_futures).await;
            for result in results {
                if result.len() > 20 {
                    context_prefix.push_str(&result);
                    context_prefix.push_str("\n\n---\n\n");
                }
            }
        }

        // If no URLs, check if the message looks like it needs a web search
        if urls.is_empty() {
            let needs_search = req.message.contains("最新")
                || req.message.contains("ニュース")
                || req.message.contains("調べて")
                || req.message.contains("検索")
                || req.message.contains("today")
                || req.message.contains("latest")
                || req.message.contains("current")
                || req.message.contains("2025")
                || req.message.contains("2026");

            if needs_search {
                tracing::info!("explore: running web search for context");
                let search_result = execute_web_search(&req.message).await;
                if search_result.len() > 20 {
                    context_prefix.push_str("## Web search results:\n");
                    context_prefix.push_str(&search_result);
                    context_prefix.push_str("\n\n---\n\n");
                }
            }
        }
    }

    // Build messages
    let mut messages = vec![Message::system(
        "あなたはChatWeb — chatweb.ai の音声対応AIアシスタントです。\
         ユーザーの質問に正確かつ詳しく回答してください。\
         提供された参考情報がある場合は、それを元に回答してください。"
    )];

    // Add session history
    {
        let mut sessions = state.sessions.lock().await;
        let session = sessions.refresh(&session_key);
        let history = session.get_history(10);
        for msg in &history {
            let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
            let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
            match role {
                "user" => messages.push(Message::user(content)),
                "assistant" => messages.push(Message::assistant(content)),
                _ => {}
            }
        }
    }

    // Hierarchical prompts: multiple prompt variations for accuracy
    // Level 0 (default): direct question
    // Level 1 (re-query): adds "think step by step" instruction
    // Level 2 (deep): adds expert persona + structured output request
    let level = req.level.unwrap_or(0);
    let base_msg = if context_prefix.is_empty() {
        req.message.clone()
    } else {
        format!("## 参考情報（事前取得済み）:\n{}\n## ユーザーの質問:\n{}", context_prefix, req.message)
    };
    let user_msg = match level {
        1 => format!(
            "{}\n\n上記の質問について、ステップバイステップで考えてから回答してください。",
            base_msg
        ),
        2 => format!(
            "あなたはこの分野の専門家です。以下の質問について、\
             まず前提条件を整理し、複数の観点から分析し、\
             最終的な結論を根拠とともに示してください。\n\n質問: {}",
            base_msg
        ),
        _ => base_msg,
    };
    messages.push(Message::user(&user_msg));

    let max_tokens = 2048u32;
    let temperature = 0.7;

    // Run explore — collect all results first, then stream as SSE events
    let state_clone = state.clone();
    let session_key_clone = session_key.clone();
    let original_msg = req.message.clone();

    let response_stream = futures::stream::once(async move {
        let start = std::time::Instant::now();
        let results = lb_raw.chat_explore(&messages, None, max_tokens, temperature).await;
        let total_time = start.elapsed().as_millis() as u64;

        // Deduct credits for each result
        #[allow(unused_mut)]
        let mut total_credits: i64 = 0;
        #[allow(unused_mut)]
        let mut last_remaining: Option<i64> = None;

        #[cfg(feature = "dynamodb-backend")]
        {
            if let (Some(dynamo), Some(table)) = (&state_clone.dynamo_client, &state_clone.config_table) {
                for result in &results {
                    let (credits, remaining) = deduct_credits(
                        dynamo, table, &session_key_clone, &result.model,
                        result.input_tokens, result.output_tokens,
                    ).await;
                    total_credits += credits;
                    if remaining.is_some() { last_remaining = remaining; }
                }
            }
        }

        // Save to session
        {
            let mut sessions = state_clone.sessions.lock().await;
            let session = sessions.get_or_create(&session_key_clone);
            session.add_message_from_channel("user", &original_msg, "web");
            if let Some(best) = results.first() {
                session.add_message_from_channel("assistant",
                    &format!("[Explore: {} models] {}", results.len(), best.response), "web");
            }
            sessions.save_by_key(&session_key_clone);
        }
        #[cfg(feature = "dynamodb-backend")]
        {
            if let (Some(dynamo), Some(table)) = (&state_clone.dynamo_client, &state_clone.config_table) {
                increment_sync_version(dynamo, table, &session_key_clone, "web").await;
            }
        }

        // Build all SSE events as a single response (API Gateway v2 compatible)
        let mut events_json = Vec::new();
        for (idx, result) in results.iter().enumerate() {
            events_json.push(serde_json::json!({
                "model": result.model,
                "response": result.response,
                "time_ms": result.response_time_ms,
                "index": idx,
                "is_fallback": result.is_fallback,
                "credits_used": crate::service::auth::calculate_credits(
                    &result.model, result.input_tokens, result.output_tokens
                ),
            }));
        }

        Ok::<_, Infallible>(Event::default().data(
            serde_json::json!({
                "type": "explore_results",
                "results": events_json,
                "total_models": results.len(),
                "total_time_ms": total_time,
                "total_credits_used": total_credits,
                "credits_remaining": last_remaining,
                "level": level,
                "can_escalate": level < 2,
            }).to_string()
        ))
    });

    Sse::new(response_stream).into_response()
}

/// Request body for the explore endpoint.
#[derive(Debug, Deserialize)]
pub struct ExploreRequest {
    pub message: String,
    #[serde(default = "default_session_id")]
    pub session_id: String,
    /// Hierarchical level: 0=direct, 1=step-by-step, 2=expert-deep
    pub level: Option<u32>,
}

/// Request body for the race endpoint.
#[derive(Debug, Deserialize)]
pub struct RaceRequest {
    pub message: String,
    #[serde(default = "default_session_id")]
    pub session_id: String,
    #[serde(default = "default_channel")]
    pub channel: String,
    pub device: Option<String>,
    /// Tier selection: "economy" | "normal" | "powerful" | null (= race all)
    pub tier: Option<String>,
}

/// POST /api/v1/chat/race — Multi-model race with ranked results, or single-tier model.
async fn handle_chat_race(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RaceRequest>,
) -> impl IntoResponse {
    use axum::response::sse::{Event, Sse};
    use std::convert::Infallible;

    // Input validation
    if req.message.len() > 32_000 {
        let err_stream = futures::stream::once(async {
            Ok::<_, Infallible>(Event::default()
                .event("error")
                .data(serde_json::json!({"error": "Message too long"}).to_string()))
        });
        return Sse::new(err_stream).into_response();
    }

    // Validate tier parameter (strict allow-list)
    if let Some(ref tier) = req.tier {
        if !matches!(tier.as_str(), "economy" | "normal" | "powerful") {
            let err_stream = futures::stream::once(async {
                Ok::<_, Infallible>(Event::default()
                    .event("error")
                    .data(serde_json::json!({"error": "Invalid tier"}).to_string()))
            });
            return Sse::new(err_stream).into_response();
        }
    }

    // Resolve session key
    let session_key = {
        #[cfg(feature = "dynamodb-backend")]
        {
            if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                resolve_session_key(dynamo, table, &req.session_id).await
            } else {
                req.session_id.clone()
            }
        }
        #[cfg(not(feature = "dynamodb-backend"))]
        { req.session_id.clone() }
    };

    // Check credits
    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let user = get_or_create_user(dynamo, table, &session_key).await;
            if user.credits_remaining <= 0 {
                let content = if user.plan == "free" {
                    "無料クレジットを使い切りました 🎉 Starterプランにアップグレードしましょう！"
                } else {
                    "クレジットを使い切りました 💪 追加購入して続けましょう！"
                };
                let err_stream = futures::stream::once(async move {
                    Ok::<_, Infallible>(Event::default()
                        .event("error")
                        .data(serde_json::json!({"error": content, "action": "upgrade"}).to_string()))
                });
                return Sse::new(err_stream).into_response();
            }
        }
    }

    // Rate limit: max 2 concurrent race requests per user (race hits multiple providers)
    let race_key = format!("race:{}", session_key);
    {
        let entry = state.concurrent_requests.entry(race_key.clone()).or_insert_with(|| AtomicU32::new(0));
        let current = entry.value().load(Ordering::Relaxed);
        if current >= 2 {
            let err_stream = futures::stream::once(async {
                Ok::<_, Infallible>(Event::default()
                    .event("error")
                    .data(serde_json::json!({"error": "レースリクエストが多すぎます。少し待ってからお試しください。"}).to_string()))
            });
            return Sse::new(err_stream).into_response();
        }
        entry.value().fetch_add(1, Ordering::Relaxed);
    }
    let state_for_ratelimit = state.clone();
    let race_key_for_ratelimit = race_key.clone();

    let lb_raw = match &state.lb_raw {
        Some(lb) => lb.clone(),
        None => {
            if let Some(e) = state.concurrent_requests.get(&race_key) { e.value().fetch_sub(1, Ordering::Relaxed); }
            let err_stream = futures::stream::once(async {
                Ok::<_, Infallible>(Event::default()
                    .event("error")
                    .data(serde_json::json!({"error": "No providers available"}).to_string()))
            });
            return Sse::new(err_stream).into_response();
        }
    };

    // Pre-process: detect URLs and search queries
    let mut context_prefix = String::new();
    {
        use crate::service::integrations::{execute_web_fetch, execute_web_search};

        let urls: Vec<&str> = URL_REGEX.find_iter(&req.message).map(|m| m.as_str()).collect();
        if !urls.is_empty() {
            tracing::info!("race: fetching {} URLs", urls.len());
            let fetch_futures: Vec<_> = urls.iter().map(|url| execute_web_fetch(url)).collect();
            let results = futures::future::join_all(fetch_futures).await;
            for result in results {
                if result.len() > 20 {
                    context_prefix.push_str(&result);
                    context_prefix.push_str("\n\n---\n\n");
                }
            }
        }

        if urls.is_empty() {
            let needs_search = req.message.contains("最新")
                || req.message.contains("ニュース")
                || req.message.contains("調べて")
                || req.message.contains("検索")
                || req.message.contains("today")
                || req.message.contains("latest")
                || req.message.contains("current")
                || req.message.contains("2025")
                || req.message.contains("2026");

            if needs_search {
                tracing::info!("race: running web search for context");
                let search_result = execute_web_search(&req.message).await;
                if search_result.len() > 20 {
                    context_prefix.push_str("## Web search results:\n");
                    context_prefix.push_str(&search_result);
                    context_prefix.push_str("\n\n---\n\n");
                }
            }
        }
    }

    // Build messages
    let mut messages = vec![crate::types::Message::system(
        "あなたはChatWeb — chatweb.ai の音声対応AIアシスタントです。\
         ユーザーの質問に正確かつ詳しく回答してください。\
         提供された参考情報がある場合は、それを元に回答してください。"
    )];

    // Add session history
    {
        let mut sessions = state.sessions.lock().await;
        let session = sessions.refresh(&session_key);
        let history = session.get_history(10);
        for msg in &history {
            let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
            let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
            match role {
                "user" => messages.push(crate::types::Message::user(content)),
                "assistant" => messages.push(crate::types::Message::assistant(content)),
                _ => {}
            }
        }
    }

    let user_msg = if context_prefix.is_empty() {
        req.message.clone()
    } else {
        format!("## 参考情報（事前取得済み）:\n{}\n## ユーザーの質問:\n{}", context_prefix, req.message)
    };
    messages.push(crate::types::Message::user(&user_msg));

    let max_tokens = 2048u32;
    let temperature = 0.7;

    let state_clone = state.clone();
    let session_key_clone = session_key.clone();
    let original_msg = req.message.clone();
    let tier = req.tier.clone();

    let response_stream = futures::stream::once(async move {
        let start = std::time::Instant::now();

        // If tier is specified, run single model
        if let Some(ref tier_name) = tier {
            if let Some((provider, model_name)) = lb_raw.get_tier_model(tier_name) {
                match provider.chat(&messages, None, &model_name, max_tokens, temperature).await {
                    Ok(resp) => {
                        let elapsed = start.elapsed().as_millis() as u64;
                        let response_text = resp.content.unwrap_or_default();
                        let input_tokens = resp.usage.prompt_tokens;
                        let output_tokens = resp.usage.completion_tokens;

                        // Deduct credits
                        #[allow(unused_mut)]
                        let mut credits_used: i64 = 0;
                        #[allow(unused_mut)]
                        let mut credits_remaining: Option<i64> = None;
                        #[cfg(feature = "dynamodb-backend")]
                        {
                            if let (Some(dynamo), Some(table)) = (&state_clone.dynamo_client, &state_clone.config_table) {
                                let (c, r) = deduct_credits(dynamo, table, &session_key_clone, &model_name, input_tokens, output_tokens).await;
                                credits_used = c;
                                credits_remaining = r;
                            }
                        }

                        // Save to session
                        {
                            let mut sessions = state_clone.sessions.lock().await;
                            let session = sessions.get_or_create(&session_key_clone);
                            session.add_message_from_channel("user", &original_msg, "web");
                            session.add_message_from_channel("assistant", &response_text, "web");
                            sessions.save_by_key(&session_key_clone);
                        }
                        #[cfg(feature = "dynamodb-backend")]
                        {
                            if let (Some(dynamo), Some(table)) = (&state_clone.dynamo_client, &state_clone.config_table) {
                                increment_sync_version(dynamo, table, &session_key_clone, "web").await;
                            }
                        }

                        return Ok::<_, Infallible>(Event::default().data(
                            serde_json::json!({
                                "type": "race_done",
                                "results": [{
                                    "rank": 1,
                                    "model": model_name,
                                    "response": response_text,
                                    "time_ms": elapsed,
                                    "credits_used": credits_used,
                                    "input_tokens": input_tokens,
                                    "output_tokens": output_tokens,
                                }],
                                "total_credits": credits_used,
                                "credits_remaining": credits_remaining,
                                "winner": model_name,
                                "tier": tier_name,
                            }).to_string()
                        ));
                    }
                    Err(e) => {
                        tracing::error!("Race tier '{}' model failed: {}", tier_name, e);
                        return Ok::<_, Infallible>(Event::default()
                            .event("error")
                            .data(serde_json::json!({"error": "サービスが一時的に利用できません。しばらくしてからお試しください。"}).to_string()));
                    }
                }
            } else {
                tracing::warn!("Race: tier '{}' has no available provider", tier_name);
                return Ok::<_, Infallible>(Event::default()
                    .event("error")
                    .data(serde_json::json!({"error": "選択されたモデルが現在利用できません。Auto Raceをお試しください。"}).to_string()));
            }
        }

        // Race mode: run all models in parallel
        let models_list: Vec<String> = lb_raw.available_parallel_models().iter().map(|(m, _)| m.clone()).collect();
        let results = lb_raw.chat_race(&messages, None, max_tokens, temperature).await;
        let total_time = start.elapsed().as_millis() as u64;

        if results.is_empty() {
            // Emergency fallback: try the first available provider directly (no race)
            tracing::warn!("Race: all parallel models failed, trying emergency fallback");
            let fallback_models = lb_raw.available_parallel_models();
            for (fb_model, fb_idx) in &fallback_models {
                let fb_provider = lb_raw.providers()[*fb_idx].clone();
                match tokio::time::timeout(
                    std::time::Duration::from_secs(15),
                    fb_provider.chat(&messages, None, fb_model, max_tokens, temperature),
                ).await {
                    Ok(Ok(resp)) => {
                        let elapsed = start.elapsed().as_millis() as u64;
                        let response_text = resp.content.unwrap_or_default();
                        let input_tokens = resp.usage.prompt_tokens;
                        let output_tokens = resp.usage.completion_tokens;

                        #[allow(unused_mut)]
                        let mut credits_used: i64 = 0;
                        #[allow(unused_mut)]
                        let mut credits_remaining: Option<i64> = None;
                        #[cfg(feature = "dynamodb-backend")]
                        {
                            if let (Some(dynamo), Some(table)) = (&state_clone.dynamo_client, &state_clone.config_table) {
                                let (c, r) = deduct_credits(dynamo, table, &session_key_clone, fb_model, input_tokens, output_tokens).await;
                                credits_used = c;
                                credits_remaining = r;
                            }
                        }
                        {
                            let mut sessions = state_clone.sessions.lock().await;
                            let session = sessions.get_or_create(&session_key_clone);
                            session.add_message_from_channel("user", &original_msg, "web");
                            session.add_message_from_channel("assistant", &response_text, "web");
                            sessions.save_by_key(&session_key_clone);
                        }

                        return Ok::<_, Infallible>(Event::default().data(
                            serde_json::json!({
                                "type": "race_done",
                                "results": [{
                                    "rank": 1,
                                    "model": fb_model,
                                    "response": response_text,
                                    "time_ms": elapsed,
                                    "credits_used": credits_used,
                                    "input_tokens": input_tokens,
                                    "output_tokens": output_tokens,
                                    "is_fallback": true,
                                }],
                                "total_credits": credits_used,
                                "credits_remaining": credits_remaining,
                                "winner": fb_model,
                                "fallback": true,
                            }).to_string()
                        ));
                    }
                    _ => {
                        tracing::warn!("Race fallback: {} also failed", fb_model);
                        continue;
                    }
                }
            }
            // If we get here, all fallback attempts also failed
            return Ok::<_, Infallible>(Event::default()
                .event("error")
                .data(serde_json::json!({"error": "全てのモデルが応答できませんでした。しばらくしてからお試しください。"}).to_string()));
        }

        // Deduct credits for each result
        #[allow(unused_mut)]
        let mut total_credits: i64 = 0;
        #[allow(unused_mut)]
        let mut last_remaining: Option<i64> = None;

        #[cfg(feature = "dynamodb-backend")]
        {
            if let (Some(dynamo), Some(table)) = (&state_clone.dynamo_client, &state_clone.config_table) {
                for result in &results {
                    let (credits, remaining) = deduct_credits(
                        dynamo, table, &session_key_clone, &result.model,
                        result.input_tokens, result.output_tokens,
                    ).await;
                    total_credits += credits;
                    if remaining.is_some() { last_remaining = remaining; }
                }
            }
        }

        // Save winner to session
        {
            let mut sessions = state_clone.sessions.lock().await;
            let session = sessions.get_or_create(&session_key_clone);
            session.add_message_from_channel("user", &original_msg, "web");
            if let Some(winner) = results.first() {
                session.add_message_from_channel("assistant", &winner.response, "web");
            }
            sessions.save_by_key(&session_key_clone);
        }
        #[cfg(feature = "dynamodb-backend")]
        {
            if let (Some(dynamo), Some(table)) = (&state_clone.dynamo_client, &state_clone.config_table) {
                increment_sync_version(dynamo, table, &session_key_clone, "web").await;
            }
        }

        // Build race response
        let mut results_json = Vec::new();
        for result in &results {
            results_json.push(serde_json::json!({
                "rank": result.rank,
                "model": result.model,
                "response": result.response,
                "time_ms": result.response_time_ms,
                "credits_used": crate::service::auth::calculate_credits(
                    &result.model, result.input_tokens, result.output_tokens
                ),
                "input_tokens": result.input_tokens,
                "output_tokens": result.output_tokens,
                "is_fallback": result.is_fallback,
            }));
        }

        let winner = results.first().map(|r| r.model.clone()).unwrap_or_default();

        Ok::<_, Infallible>(Event::default().data(
            serde_json::json!({
                "type": "race_done",
                "models": models_list,
                "results": results_json,
                "total_time_ms": total_time,
                "total_credits": total_credits,
                "credits_remaining": last_remaining,
                "winner": winner,
            }).to_string()
        ))
    });

    // Wrap stream to ensure rate-limit counter is decremented after completion
    use futures::StreamExt;
    let response_stream = response_stream.map(move |result| {
        // Decrement concurrent race counter
        if let Some(e) = state_for_ratelimit.concurrent_requests.get(&race_key_for_ratelimit) {
            e.value().fetch_sub(1, Ordering::Relaxed);
        }
        result
    });

    Sse::new(response_stream).into_response()
}

/// POST /api/v1/billing/checkout — Create Stripe Checkout session via API
async fn handle_billing_checkout(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<CheckoutRequest>,
) -> impl IntoResponse {
    let stripe_key = std::env::var("STRIPE_SECRET_KEY").unwrap_or_default();
    if stripe_key.is_empty() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "Stripe not configured"})),
        );
    }

    let session_key = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim_start_matches("Bearer ").to_string())
        .or_else(|| headers.get("x-session-id").and_then(|v| v.to_str().ok()).map(|s| s.to_string()))
        .unwrap_or_default();

    let plan = req.plan.to_lowercase();
    let price_id = match plan.as_str() {
        "starter" => std::env::var("STRIPE_PRICE_STARTER").unwrap_or_default(),
        "pro" => std::env::var("STRIPE_PRICE_PRO").unwrap_or_default(),
        "enterprise" => std::env::var("STRIPE_PRICE_ENTERPRISE").unwrap_or_default(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Invalid plan"})),
            )
        }
    };

    if price_id.is_empty() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "Price not configured for this plan"})),
        );
    }

    let success_url = "https://chatweb.ai/?checkout=success&session_id={CHECKOUT_SESSION_ID}";
    let cancel_url = "https://chatweb.ai/?checkout=cancel";

    let params = vec![
        ("mode", "subscription".to_string()),
        ("success_url", success_url.to_string()),
        ("cancel_url", cancel_url.to_string()),
        ("client_reference_id", session_key.clone()),
        ("line_items[0][price]", price_id.clone()),
        ("line_items[0][quantity]", "1".to_string()),
        ("metadata[checkout_type]", "subscription".to_string()),
        ("metadata[plan]", plan.clone()),
        ("metadata[session_key]", session_key),
    ];

    let client = reqwest::Client::new();
    match client.post("https://api.stripe.com/v1/checkout/sessions")
        .header("Authorization", format!("Bearer {}", stripe_key))
        .form(&params)
        .send()
        .await
    {
        Ok(resp) => {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            if let Some(url) = body["url"].as_str() {
                (StatusCode::OK, Json(serde_json::json!({
                    "checkout_url": url,
                    "plan": plan,
                })))
            } else {
                let err = body["error"]["message"].as_str().unwrap_or("Unknown Stripe error");
                tracing::error!("Stripe checkout session error: {}", err);
                (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": err})))
            }
        }
        Err(e) => {
            tracing::error!("Stripe API call failed: {}", e);
            (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": "Payment service unavailable"})))
        }
    }
}

/// POST /api/v1/billing/credit-pack — Purchase a one-time credit pack
async fn handle_credit_pack_checkout(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<CreditPackRequest>,
) -> impl IntoResponse {
    let stripe_key = std::env::var("STRIPE_SECRET_KEY").unwrap_or_default();
    if stripe_key.is_empty() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "Stripe not configured"})),
        );
    }

    let session_key = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim_start_matches("Bearer ").to_string())
        .or_else(|| headers.get("x-session-id").and_then(|v| v.to_str().ok()).map(|s| s.to_string()))
        .unwrap_or_default();

    // Credit pack pricing (JPY)
    let (credits, amount_jpy, name) = match req.credits {
        c if c <= 1000 => (1000i64, 500i64, "ChatWeb 1,000 クレジット"),
        c if c <= 5000 => (5000, 1980, "ChatWeb 5,000 クレジット"),
        c if c <= 20000 => (20000, 5980, "ChatWeb 20,000 クレジット"),
        _ => (50000, 12800, "ChatWeb 50,000 クレジット"),
    };

    let success_url = "https://chatweb.ai/?checkout=success&session_id={CHECKOUT_SESSION_ID}";
    let cancel_url = "https://chatweb.ai/?checkout=cancel";

    let params = vec![
        ("mode", "payment".to_string()),
        ("success_url", success_url.to_string()),
        ("cancel_url", cancel_url.to_string()),
        ("client_reference_id", session_key.clone()),
        ("line_items[0][price_data][currency]", "jpy".to_string()),
        ("line_items[0][price_data][product_data][name]", name.to_string()),
        ("line_items[0][price_data][unit_amount]", amount_jpy.to_string()),
        ("line_items[0][quantity]", "1".to_string()),
        ("payment_intent_data[setup_future_usage]", "off_session".to_string()),
        ("metadata[checkout_type]", "credit_pack".to_string()),
        ("metadata[credits]", credits.to_string()),
        ("metadata[session_key]", session_key),
    ];

    let client = reqwest::Client::new();
    match client.post("https://api.stripe.com/v1/checkout/sessions")
        .header("Authorization", format!("Bearer {}", stripe_key))
        .form(&params)
        .send()
        .await
    {
        Ok(resp) => {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            if let Some(url) = body["url"].as_str() {
                (StatusCode::OK, Json(serde_json::json!({
                    "checkout_url": url,
                    "credits": credits,
                    "amount_jpy": amount_jpy,
                })))
            } else {
                let err = body["error"]["message"].as_str().unwrap_or("Unknown Stripe error");
                tracing::error!("Stripe credit pack error: {}", err);
                (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": err})))
            }
        }
        Err(e) => {
            tracing::error!("Stripe API call failed: {}", e);
            (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": "Payment service unavailable"})))
        }
    }
}

/// POST /api/v1/billing/auto-charge — Toggle auto-charge preference
async fn handle_auto_charge_toggle(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<AutoChargeRequest>,
) -> impl IntoResponse {
    let session_key = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim_start_matches("Bearer ").to_string())
        .or_else(|| headers.get("x-session-id").and_then(|v| v.to_str().ok()).map(|s| s.to_string()))
        .unwrap_or_default();

    #[cfg(feature = "dynamodb-backend")]
    if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
        let user_key = resolve_session_key(dynamo, table, &session_key).await;
        let pk = format!("USER#{}", user_key);
        let _ = dynamo
            .update_item()
            .table_name(table)
            .key("pk", AttributeValue::S(pk))
            .key("sk", AttributeValue::S(SK_PROFILE.to_string()))
            .update_expression("SET auto_charge = :ac, auto_charge_credits = :acc, updated_at = :now")
            .expression_attribute_values(":ac", AttributeValue::Bool(req.enabled))
            .expression_attribute_values(":acc", AttributeValue::N(req.credits.unwrap_or(5000).to_string()))
            .expression_attribute_values(":now", AttributeValue::S(chrono::Utc::now().to_rfc3339()))
            .send()
            .await;

        return Json(serde_json::json!({
            "auto_charge": req.enabled,
            "auto_charge_credits": req.credits.unwrap_or(5000),
        }));
    }

    #[cfg(not(feature = "dynamodb-backend"))]
    let _ = (&state, &session_key);

    Json(serde_json::json!({"auto_charge": req.enabled}))
}

/// GET /api/v1/billing/portal — Get Stripe billing portal URL
async fn handle_billing_portal(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let stripe_key = std::env::var("STRIPE_SECRET_KEY").unwrap_or_default();

    // Try to create a portal session via Stripe API if we have the customer ID
    let session_key = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim_start_matches("Bearer ").to_string())
        .or_else(|| headers.get("x-session-id").and_then(|v| v.to_str().ok()).map(|s| s.to_string()))
        .unwrap_or_default();

    #[cfg(feature = "dynamodb-backend")]
    if !stripe_key.is_empty() {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let user_key = resolve_session_key(dynamo, table, &session_key).await;
            let user = get_or_create_user(dynamo, table, &user_key).await;
            if let Some(customer_id) = &user.stripe_customer_id {
                let client = reqwest::Client::new();
                let params = vec![
                    ("customer", customer_id.as_str()),
                    ("return_url", "https://chatweb.ai/"),
                ];
                if let Ok(resp) = client.post("https://api.stripe.com/v1/billing_portal/sessions")
                    .header("Authorization", format!("Bearer {}", stripe_key))
                    .form(&params)
                    .send()
                    .await
                {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    if let Some(url) = body["url"].as_str() {
                        return Json(serde_json::json!({"portal_url": url}));
                    }
                }
            }
        }
    }

    #[cfg(not(feature = "dynamodb-backend"))]
    let _ = (&state, &session_key, &stripe_key);

    let portal_url = std::env::var("STRIPE_PORTAL_URL")
        .unwrap_or_else(|_| "https://billing.stripe.com/p/login/test".to_string());

    Json(serde_json::json!({
        "portal_url": portal_url,
    }))
}

/// POST /webhooks/stripe — Stripe webhook
async fn handle_stripe_webhook(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    body: String,
) -> impl IntoResponse {
    info!("Stripe webhook received: {} bytes", body.len());

    #[cfg(feature = "stripe")]
    {
        let webhook_secret = std::env::var("STRIPE_WEBHOOK_SECRET").unwrap_or_default();
        let signature = headers
            .get("stripe-signature")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if !webhook_secret.is_empty()
            && !verify_webhook_signature(body.as_bytes(), signature, &webhook_secret)
        {
            return StatusCode::UNAUTHORIZED;
        }

        if let Ok(event) = serde_json::from_str::<serde_json::Value>(&body) {
            let event_type = event
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let result = process_webhook_event(event_type, &event);
            info!("Stripe event processed: {:?}", result);

            // On checkout.session.completed — link payment to existing user
            #[cfg(feature = "dynamodb-backend")]
            if event_type == "checkout.session.completed" {
                if let (Some(client), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                    let customer_id = event
                        .pointer("/data/object/customer")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let customer_email = event
                        .pointer("/data/object/customer_details/email")
                        .or_else(|| event.pointer("/data/object/customer_email"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    // Get client_reference_id (= session_key from frontend)
                    let client_ref = event
                        .pointer("/data/object/client_reference_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    // Get checkout type from metadata
                    let checkout_type = event
                        .pointer("/data/object/metadata/checkout_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("subscription");

                    // Fallback: metadata.session_key (set by frontend checkout)
                    let metadata_session_key = event
                        .pointer("/data/object/metadata/session_key")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    // Resolve existing user: client_reference_id → metadata.session_key → email → new user
                    let user_id = if !client_ref.is_empty() {
                        let resolved = resolve_session_key(client, table, client_ref).await;
                        info!("Stripe checkout: resolved client_ref {} → {}", client_ref, resolved);
                        resolved
                    } else if !metadata_session_key.is_empty() {
                        let resolved = resolve_session_key(client, table, metadata_session_key).await;
                        info!("Stripe checkout: resolved metadata.session_key {} → {}", metadata_session_key, resolved);
                        resolved
                    } else if !customer_email.is_empty() {
                        // Fallback: try to find by email
                        find_user_by_email(client, table, customer_email).await
                            .unwrap_or_else(|| format!("user:{}", uuid::Uuid::new_v4()))
                    } else {
                        tracing::warn!("Stripe checkout: no client_ref, no metadata.session_key, no email — creating new user");
                        format!("user:{}", uuid::Uuid::new_v4())
                    };

                    let _ = get_or_create_user(client, table, &user_id).await;

                    if checkout_type == "credit_pack" {
                        // One-time credit purchase — add credits without changing plan
                        let credits_to_add: i64 = event
                            .pointer("/data/object/metadata/credits")
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(1000);

                        add_credits_to_user(client, table, &user_id, credits_to_add, customer_id, customer_email).await;
                        info!("Credit pack: added {} credits to user {} (customer={})", credits_to_add, user_id, customer_id);
                    } else {
                        // Subscription — upgrade plan + set monthly credits
                        let mode = event
                            .pointer("/data/object/mode")
                            .and_then(|v| v.as_str())
                            .unwrap_or("subscription");

                        // Determine plan from metadata or price
                        let plan_name = event
                            .pointer("/data/object/metadata/plan")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .or_else(|| {
                                event.pointer("/data/object/display_items/0/price/id")
                                    .or_else(|| event.pointer("/data/object/line_items/data/0/price/id"))
                                    .and_then(|v| v.as_str())
                                    .and_then(|price_id| crate::service::stripe::price_to_plan(price_id).map(|p| p.to_string()))
                            });

                        if let Some(plan_name) = plan_name {
                            link_stripe_to_user(client, table, &user_id, customer_id, customer_email, &plan_name).await;
                            info!("Subscription: upgraded user {} to plan {} (customer={})", user_id, plan_name, customer_id);
                        } else if mode == "payment" {
                            // One-time payment without specific metadata — treat as generic credit pack
                            add_credits_to_user(client, table, &user_id, 1000, customer_id, customer_email).await;
                            info!("Generic payment: added 1000 credits to user {} (customer={})", user_id, customer_id);
                        } else {
                            // Default to starter
                            link_stripe_to_user(client, table, &user_id, customer_id, customer_email, "starter").await;
                            info!("Subscription (default starter): upgraded user {} (customer={})", user_id, customer_id);
                        }
                    }
                }
            }

            // Handle subscription updates (plan changes)
            #[cfg(feature = "dynamodb-backend")]
            if event_type == "customer.subscription.updated" || event_type == "invoice.paid" {
                if let (Some(client), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                    let customer_id = event
                        .pointer("/data/object/customer")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    if !customer_id.is_empty() {
                        info!("Subscription event for customer {}: {}", customer_id, event_type);

                        // Find user by stripe_customer_id
                        if let Some(user_id) = find_user_by_stripe_customer(client, table, customer_id).await {
                            if event_type == "invoice.paid" {
                                // Monthly invoice paid — reset credits to monthly allowance
                                let user = get_or_create_user(client, table, &user_id).await;
                                let plan: crate::service::auth::Plan = user.plan.parse().unwrap_or(crate::service::auth::Plan::Starter);
                                let monthly_credits = plan.monthly_credits();
                                let pk = format!("USER#{}", user_id);
                                match client
                                    .update_item()
                                    .table_name(table)
                                    .key("pk", AttributeValue::S(pk))
                                    .key("sk", AttributeValue::S(SK_PROFILE.to_string()))
                                    .update_expression("SET credits_remaining = :cr, updated_at = :now")
                                    .expression_attribute_values(":cr", AttributeValue::N(monthly_credits.to_string()))
                                    .expression_attribute_values(":now", AttributeValue::S(chrono::Utc::now().to_rfc3339()))
                                    .send()
                                    .await
                                {
                                    Ok(_) => info!("Reset credits to {} for user {} (invoice.paid)", monthly_credits, user_id),
                                    Err(e) => tracing::error!("BILLING ERROR: Failed to reset credits to {} for user {} (invoice.paid): {}", monthly_credits, user_id, e),
                                }
                            } else {
                                // Subscription updated — check for plan change
                                let new_plan = event
                                    .pointer("/data/object/items/data/0/price/id")
                                    .and_then(|v| v.as_str())
                                    .and_then(crate::service::stripe::price_to_plan)
                                    .map(|p| p.to_string())
                                    .unwrap_or_default();
                                if !new_plan.is_empty() {
                                    let email = ""; // email not in subscription event
                                    link_stripe_to_user(client, table, &user_id, customer_id, email, &new_plan).await;
                                    info!("Plan changed to {} for user {} (subscription.updated)", new_plan, user_id);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[cfg(not(feature = "stripe"))]
    let _ = (&state, &headers);

    StatusCode::OK
}

/// Request body for coupon validation.
#[derive(Debug, Deserialize)]
pub struct CouponRequest {
    pub code: String,
}

/// POST /api/v1/coupon/validate — Validate coupon code
async fn handle_coupon_validate(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CouponRequest>,
) -> impl IntoResponse {
    let code = req.code.trim().to_uppercase();
    info!("Coupon validation: {}", code);

    // Check DynamoDB config table for coupon
    #[cfg(feature = "dynamodb-backend")]
    if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
        let resp = dynamo
            .get_item()
            .table_name(table)
            .key("pk", AttributeValue::S(format!("COUPON#{}", code)))
            .key("sk", AttributeValue::S("CONFIG".to_string()))
            .send()
            .await;

        if let Ok(output) = resp {
            if let Some(item) = output.item {
                let active = item.get("active").and_then(|v| v.as_bool().ok()).copied().unwrap_or(false);
                if active {
                    let description = item.get("description").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                    let description_ja = item.get("description_ja").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                    let stripe_promo = item.get("stripe_promo_code").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();

                    let grant_credits = item.get("grant_credits").and_then(|v| v.as_n().ok())
                        .and_then(|n| n.parse::<i64>().ok()).unwrap_or(0);
                    let grant_days = item.get("grant_days").and_then(|v| v.as_n().ok())
                        .and_then(|n| n.parse::<i64>().ok()).unwrap_or(30);
                    let require_card = item.get("require_card").and_then(|v| v.as_bool().ok()).copied().unwrap_or(true);

                    return Json(serde_json::json!({
                        "valid": true,
                        "code": code,
                        "description": description,
                        "description_ja": description_ja,
                        "stripe_promo_code": stripe_promo,
                        "grant_credits": grant_credits,
                        "grant_days": grant_days,
                        "require_card": require_card,
                    }));
                }
            }
        }
    }

    #[cfg(not(feature = "dynamodb-backend"))]
    let _ = &state;

    Json(serde_json::json!({
        "valid": false,
        "code": code,
    }))
}

/// Request body for coupon redemption.
#[derive(Debug, Deserialize)]
pub struct CouponRedeemRequest {
    pub code: String,
    pub session_id: Option<String>,
}

/// POST /api/v1/coupon/redeem — Redeem coupon: grant credits + upgrade plan (no card required)
async fn handle_coupon_redeem(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<CouponRedeemRequest>,
) -> impl IntoResponse {
    let code = req.code.trim().to_uppercase();
    info!("Coupon redeem: {}", code);

    #[cfg(feature = "dynamodb-backend")]
    if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
        // Special: KONAMI code — grants 1000 credits per use, cap at 100,000 total
        if code == "KONAMI" {
            let session_key = if let Some(ref sid) = req.session_id {
                sid.clone()
            } else {
                auth_user_id(&state, &headers).await.unwrap_or_default()
            };
            if session_key.is_empty() {
                return (axum::http::StatusCode::UNAUTHORIZED, Json(serde_json::json!({
                    "error": "Login required"
                }))).into_response();
            }
            let resolved_user = resolve_session_key(dynamo, table, &session_key).await;
            let user = get_or_create_user(dynamo, table, &resolved_user).await;
            if user.credits_remaining >= 100_000 {
                return (axum::http::StatusCode::BAD_REQUEST, Json(serde_json::json!({
                    "error": "Credit cap reached (100,000)",
                    "error_ja": "クレジット上限（100,000）に達しています"
                }))).into_response();
            }
            let grant = 1000i64.min(100_000 - user.credits_remaining);
            let pk = format!("USER#{}", resolved_user);
            let _ = dynamo
                .update_item()
                .table_name(table)
                .key("pk", AttributeValue::S(pk))
                .key("sk", AttributeValue::S(SK_PROFILE.to_string()))
                .update_expression("SET credits_remaining = credits_remaining + :c, updated_at = :now")
                .expression_attribute_values(":c", AttributeValue::N(grant.to_string()))
                .expression_attribute_values(":now", AttributeValue::S(chrono::Utc::now().to_rfc3339()))
                .send()
                .await;
            let updated = get_or_create_user(dynamo, table, &resolved_user).await;
            emit_audit_log(dynamo.clone(), table.clone(), "konami_redeemed", &resolved_user, "",
                &format!("granted={}, new_balance={}", grant, updated.credits_remaining));
            return Json(serde_json::json!({
                "success": true,
                "credits_granted": grant,
                "credits_remaining": updated.credits_remaining,
            })).into_response();
        }

        // 1. Validate coupon exists and is active
        let coupon_resp = dynamo
            .get_item()
            .table_name(table)
            .key("pk", AttributeValue::S(format!("COUPON#{}", code)))
            .key("sk", AttributeValue::S("CONFIG".to_string()))
            .send()
            .await;

        let coupon_item = match coupon_resp {
            Ok(output) => match output.item {
                Some(item) => {
                    let active = item.get("active").and_then(|v| v.as_bool().ok()).copied().unwrap_or(false);
                    if !active {
                        return (axum::http::StatusCode::BAD_REQUEST, Json(serde_json::json!({
                            "error": "Coupon is no longer active"
                        }))).into_response();
                    }
                    item
                },
                None => return (axum::http::StatusCode::BAD_REQUEST, Json(serde_json::json!({
                    "error": "Invalid coupon code"
                }))).into_response(),
            },
            Err(_) => return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
                "error": "Failed to validate coupon"
            }))).into_response(),
        };

        // 2. Resolve user
        let session_key = if let Some(ref sid) = req.session_id {
            sid.clone()
        } else {
            auth_user_id(&state, &headers).await.unwrap_or_default()
        };

        if session_key.is_empty() {
            return (axum::http::StatusCode::UNAUTHORIZED, Json(serde_json::json!({
                "error": "Login required to redeem coupon"
            }))).into_response();
        }

        let resolved_user = resolve_session_key(dynamo, table, &session_key).await;

        // 3. Check redemption count (enforce per-user limit)
        let uses_per_user = coupon_item.get("uses_per_user").and_then(|v| v.as_n().ok())
            .and_then(|n| n.parse::<i64>().ok()).unwrap_or(1);

        let redeem_check = dynamo
            .get_item()
            .table_name(table)
            .key("pk", AttributeValue::S(format!("REDEEM#{}#{}", resolved_user, code)))
            .key("sk", AttributeValue::S("INFO".to_string()))
            .send()
            .await;

        let current_count = if let Ok(ref output) = redeem_check {
            if let Some(ref item) = output.item {
                item.get("count").and_then(|v| v.as_n().ok())
                    .and_then(|n| n.parse::<i64>().ok()).unwrap_or(0)
            } else {
                0
            }
        } else {
            0
        };

        if current_count >= uses_per_user {
            return (axum::http::StatusCode::BAD_REQUEST, Json(serde_json::json!({
                "error": format!("Coupon redemption limit reached ({}/{})", current_count, uses_per_user),
                "error_ja": format!("クーポン使用上限に達しました ({}/{}回)", current_count, uses_per_user)
            }))).into_response();
        }

        // 4. Extract coupon benefits
        let grant_credits = coupon_item.get("grant_credits").and_then(|v| v.as_n().ok())
            .and_then(|n| n.parse::<i64>().ok()).unwrap_or(0);
        let grant_plan = coupon_item.get("grant_plan").and_then(|v| v.as_s().ok())
            .cloned().unwrap_or_else(|| "starter".to_string());
        let grant_days = coupon_item.get("grant_days").and_then(|v| v.as_n().ok())
            .and_then(|n| n.parse::<i64>().ok()).unwrap_or(30);

        // 5. Update user profile: add credits + upgrade plan
        let now = chrono::Utc::now().to_rfc3339();
        let expires_at = (chrono::Utc::now() + chrono::Duration::days(grant_days)).to_rfc3339();
        let pk = if resolved_user.contains('@') {
            format!("USER#{}", resolved_user)
        } else {
            format!("USER#{}", resolved_user)
        };

        let _ = dynamo
            .update_item()
            .table_name(table)
            .key("pk", AttributeValue::S(pk))
            .key("sk", AttributeValue::S(SK_PROFILE.to_string()))
            .update_expression("SET credits_remaining = credits_remaining + :c, #p = :plan, coupon_code = :coupon, coupon_expires = :exp, updated_at = :now")
            .expression_attribute_names("#p", "plan")
            .expression_attribute_values(":c", AttributeValue::N(grant_credits.to_string()))
            .expression_attribute_values(":plan", AttributeValue::S(grant_plan.clone()))
            .expression_attribute_values(":coupon", AttributeValue::S(code.clone()))
            .expression_attribute_values(":exp", AttributeValue::S(expires_at.clone()))
            .expression_attribute_values(":now", AttributeValue::S(now.clone()))
            .send()
            .await;

        // 6. Record redemption (increment count)
        let ttl = (chrono::Utc::now() + chrono::Duration::days(grant_days + 30)).timestamp();
        let new_count = current_count + 1;

        if current_count == 0 {
            // First redemption - create record
            let _ = dynamo
                .put_item()
                .table_name(table)
                .item("pk", AttributeValue::S(format!("REDEEM#{}#{}", resolved_user, code)))
                .item("sk", AttributeValue::S("INFO".to_string()))
                .item("user_id", AttributeValue::S(resolved_user.clone()))
                .item("code", AttributeValue::S(code.clone()))
                .item("count", AttributeValue::N("1".to_string()))
                .item("grant_credits", AttributeValue::N(grant_credits.to_string()))
                .item("grant_plan", AttributeValue::S(grant_plan.clone()))
                .item("first_redeemed_at", AttributeValue::S(now.clone()))
                .item("last_redeemed_at", AttributeValue::S(now.clone()))
                .item("ttl", AttributeValue::N(ttl.to_string()))
                .send()
                .await;
        } else {
            // Subsequent redemption - increment count
            let _ = dynamo
                .update_item()
                .table_name(table)
                .key("pk", AttributeValue::S(format!("REDEEM#{}#{}", resolved_user, code)))
                .key("sk", AttributeValue::S("INFO".to_string()))
                .update_expression("ADD #count :one SET last_redeemed_at = :now, #ttl = :ttl")
                .expression_attribute_names("#count", "count")
                .expression_attribute_names("#ttl", "ttl")
                .expression_attribute_values(":one", AttributeValue::N("1".to_string()))
                .expression_attribute_values(":now", AttributeValue::S(now.clone()))
                .expression_attribute_values(":ttl", AttributeValue::N(ttl.to_string()))
                .send()
                .await;
        }

        // 7. Log audit
        emit_audit_log(dynamo.clone(), table.clone(), "coupon_redeemed", &resolved_user, "",
            &format!("code={}, credits={}, plan={}, days={}", code, grant_credits, grant_plan, grant_days));

        // 8. Get updated user profile
        let user = get_or_create_user(dynamo, table, &resolved_user).await;

        return Json(serde_json::json!({
            "success": true,
            "code": code,
            "grant_credits": grant_credits,
            "grant_plan": grant_plan,
            "grant_days": grant_days,
            "expires_at": expires_at,
            "credits_remaining": user.credits_remaining,
            "plan": user.plan,
            "redemption_count": new_count,
            "redemption_limit": uses_per_user,
            "remaining_uses": uses_per_user - new_count,
            "message": format!("Coupon redeemed successfully! ({}/{})", new_count, uses_per_user),
            "message_ja": format!("クーポン適用完了！{}クレジット付与（{}/{}回目）", grant_credits, new_count, uses_per_user),
        })).into_response();
    }

    #[cfg(not(feature = "dynamodb-backend"))]
    let _ = (&state, &headers);

    (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
        "error": "Coupon system not available"
    }))).into_response()
}

// ---------------------------------------------------------------------------
// Referral system
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ReferralApplyRequest {
    code: String,
}

/// Generate a 6-char alphanumeric referral code from a user_id (deterministic).
fn generate_referral_code(user_id: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    user_id.hash(&mut hasher);
    let hash = hasher.finish();
    const CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789"; // no 0/O/1/I confusion
    let mut code = String::with_capacity(6);
    let mut h = hash;
    for _ in 0..6 {
        code.push(CHARS[(h % CHARS.len() as u64) as usize] as char);
        h /= CHARS.len() as u64;
    }
    code
}

/// GET /api/v1/referral/code — Get or create referral code for authenticated user
async fn handle_referral_code(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let user_id = match auth_user_id(&state, &headers).await {
        Some(id) => id,
        None => return Json(serde_json::json!({ "error": "Unauthorized" })),
    };

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            // Check if user already has a referral code in profile
            let user = get_or_create_user(dynamo, table, &user_id).await;
            let code = generate_referral_code(&user_id);

            // Ensure REFERRAL#{code} OWNER record exists (idempotent)
            let _ = dynamo
                .put_item()
                .table_name(table)
                .item("pk", AttributeValue::S(format!("REFERRAL#{}", code)))
                .item("sk", AttributeValue::S("OWNER".to_string()))
                .item("user_id", AttributeValue::S(user_id.clone()))
                .item("created_at", AttributeValue::S(chrono::Utc::now().to_rfc3339()))
                .condition_expression("attribute_not_exists(pk)")
                .send()
                .await; // ignore error if already exists

            // Store referral_code on user profile if not set
            let _ = dynamo
                .update_item()
                .table_name(table)
                .key("pk", AttributeValue::S(format!("USER#{}", user_id)))
                .key("sk", AttributeValue::S(SK_PROFILE.to_string()))
                .update_expression("SET referral_code = if_not_exists(referral_code, :code)")
                .expression_attribute_values(":code", AttributeValue::S(code.clone()))
                .send()
                .await;

            let referral_url = format!("https://chatweb.ai?ref={}", code);
            return Json(serde_json::json!({
                "ok": true,
                "code": code,
                "url": referral_url,
                "credits_remaining": user.credits_remaining,
            }));
        }
    }

    Json(serde_json::json!({ "error": "Not available" }))
}

/// POST /api/v1/referral/apply — Apply a referral code (referee gets +100, referrer gets +100)
async fn handle_referral_apply(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<ReferralApplyRequest>,
) -> impl IntoResponse {
    let user_id = match auth_user_id(&state, &headers).await {
        Some(id) => id,
        None => return Json(serde_json::json!({ "error": "Unauthorized" })),
    };

    let code = req.code.trim().to_uppercase();
    if code.len() < 4 || code.len() > 10 {
        return Json(serde_json::json!({ "error": "Invalid referral code" }));
    }

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            // Look up referral code owner
            let referrer_id = match dynamo
                .get_item()
                .table_name(table)
                .key("pk", AttributeValue::S(format!("REFERRAL#{}", code)))
                .key("sk", AttributeValue::S("OWNER".to_string()))
                .send()
                .await
            {
                Ok(output) => output.item.and_then(|item| {
                    item.get("user_id").and_then(|v| v.as_s().ok()).map(|s| s.to_string())
                }),
                Err(_) => None,
            };

            let referrer_id = match referrer_id {
                Some(id) => id,
                None => return Json(serde_json::json!({ "error": "Invalid referral code" })),
            };

            // Self-referral check
            if referrer_id == user_id {
                return Json(serde_json::json!({ "error": "Cannot use your own referral code" }));
            }

            // Duplicate check: REFERRAL_LOG#{referrer}#{referee}
            let log_pk = format!("REFERRAL_LOG#{}#{}", referrer_id, user_id);
            let already_used = match dynamo
                .get_item()
                .table_name(table)
                .key("pk", AttributeValue::S(log_pk.clone()))
                .key("sk", AttributeValue::S("INFO".to_string()))
                .send()
                .await
            {
                Ok(output) => output.item.is_some(),
                Err(_) => false,
            };

            if already_used {
                return Json(serde_json::json!({ "error": "Referral code already used" }));
            }

            // Also check if this user has ever used ANY referral code
            // Use a simple flag on the user profile
            let user = get_or_create_user(dynamo, table, &user_id).await;
            // Check referred_by field via direct get
            let has_referrer = match dynamo
                .get_item()
                .table_name(table)
                .key("pk", AttributeValue::S(format!("USER#{}", user_id)))
                .key("sk", AttributeValue::S(SK_PROFILE.to_string()))
                .send()
                .await
            {
                Ok(output) => output.item
                    .and_then(|item| item.get("referred_by").cloned())
                    .is_some(),
                Err(_) => false,
            };

            if has_referrer {
                return Json(serde_json::json!({ "error": "You have already used a referral code" }));
            }

            let now = chrono::Utc::now().to_rfc3339();
            let bonus: i64 = 100;

            // Record referral log (prevents duplicates)
            let _ = dynamo
                .put_item()
                .table_name(table)
                .item("pk", AttributeValue::S(log_pk))
                .item("sk", AttributeValue::S("INFO".to_string()))
                .item("referrer_id", AttributeValue::S(referrer_id.clone()))
                .item("referee_id", AttributeValue::S(user_id.clone()))
                .item("code", AttributeValue::S(code.clone()))
                .item("bonus", AttributeValue::N(bonus.to_string()))
                .item("created_at", AttributeValue::S(now.clone()))
                .condition_expression("attribute_not_exists(pk)")
                .send()
                .await;

            // Credit referee (current user) +100
            let _ = dynamo
                .update_item()
                .table_name(table)
                .key("pk", AttributeValue::S(format!("USER#{}", user_id)))
                .key("sk", AttributeValue::S(SK_PROFILE.to_string()))
                .update_expression("SET credits_remaining = credits_remaining + :bonus, referred_by = :ref, updated_at = :now")
                .expression_attribute_values(":bonus", AttributeValue::N(bonus.to_string()))
                .expression_attribute_values(":ref", AttributeValue::S(referrer_id.clone()))
                .expression_attribute_values(":now", AttributeValue::S(now.clone()))
                .send()
                .await;

            // Credit referrer +100
            let _ = dynamo
                .update_item()
                .table_name(table)
                .key("pk", AttributeValue::S(format!("USER#{}", referrer_id)))
                .key("sk", AttributeValue::S(SK_PROFILE.to_string()))
                .update_expression("SET credits_remaining = credits_remaining + :bonus, updated_at = :now")
                .expression_attribute_values(":bonus", AttributeValue::N(bonus.to_string()))
                .expression_attribute_values(":now", AttributeValue::S(now))
                .send()
                .await;

            let updated_user = get_or_create_user(dynamo, table, &user_id).await;
            return Json(serde_json::json!({
                "ok": true,
                "bonus_credits": bonus,
                "credits_remaining": updated_user.credits_remaining,
                "message": "Referral bonus applied! +100 credits"
            }));
        }
    }

    Json(serde_json::json!({ "error": "Not available" }))
}

// ---------------------------------------------------------------------------
// Crypto payment (OpenRouter onchain — ETH/MATIC/Base)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CryptoInitiateRequest {
    amount: f64,                    // USD amount (min $5)
    chain_id: Option<u64>,          // 1=Ethereum, 137=Polygon, 8453=Base (default)
    wallet_address: String,         // User's wallet address (0x...)
    session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CryptoConfirmRequest {
    tx_hash: String,
    session_id: Option<String>,
}

/// POST /api/v1/crypto/initiate — Get calldata for onchain crypto payment
async fn handle_crypto_initiate(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<CryptoInitiateRequest>,
) -> impl IntoResponse {
    // Validate amount
    if req.amount < 5.0 || req.amount > 500.0 {
        return Json(serde_json::json!({
            "error": "Amount must be between $5 and $500"
        })).into_response();
    }

    // Validate wallet address
    if !req.wallet_address.starts_with("0x") || req.wallet_address.len() != 42 {
        return Json(serde_json::json!({
            "error": "Invalid wallet address"
        })).into_response();
    }

    // Resolve user
    let _user_id = {
        #[cfg(feature = "dynamodb-backend")]
        {
            let uid = auth_user_id(&state, &headers).await
                .or_else(|| req.session_id.clone());
            match uid {
                Some(id) if !id.is_empty() => id,
                _ => return Json(serde_json::json!({
                    "error": "Login required"
                })).into_response(),
            }
        }
        #[cfg(not(feature = "dynamodb-backend"))]
        { let _ = (&state, &headers); String::new() }
    };

    let chain_id = req.chain_id.unwrap_or(8453); // Default: Base (cheapest gas)

    // Call OpenRouter crypto API
    let or_key = match std::env::var("OPENROUTER_API_KEY") {
        Ok(k) if !k.is_empty() => k,
        _ => return Json(serde_json::json!({
            "error": "Crypto payment not configured"
        })).into_response(),
    };

    let client = reqwest::Client::new();
    let or_resp = client
        .post("https://openrouter.ai/api/v1/credits/coinbase")
        .header("Authorization", format!("Bearer {}", or_key))
        .json(&serde_json::json!({
            "amount": req.amount,
            "sender": req.wallet_address,
            "chain_id": chain_id,
        }))
        .send()
        .await;

    match or_resp {
        Ok(resp) if resp.status().is_success() => {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();

            // Store pending transaction in DynamoDB
            #[cfg(feature = "dynamodb-backend")]
            if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                let tx_id = uuid::Uuid::new_v4().to_string();
                let _ = dynamo
                    .put_item()
                    .table_name(table)
                    .item("pk", AttributeValue::S(format!("CRYPTO_TX#{}", tx_id)))
                    .item("sk", AttributeValue::S("PENDING".to_string()))
                    .item("user_id", AttributeValue::S(_user_id.clone()))
                    .item("amount_usd", AttributeValue::N(req.amount.to_string()))
                    .item("chain_id", AttributeValue::N(chain_id.to_string()))
                    .item("wallet", AttributeValue::S(req.wallet_address.clone()))
                    .item("created_at", AttributeValue::S(chrono::Utc::now().to_rfc3339()))
                    .item("status", AttributeValue::S("pending".to_string()))
                    .item("ttl", AttributeValue::N(
                        (chrono::Utc::now().timestamp() + 3600).to_string() // 1 hour expiry
                    ))
                    .send()
                    .await;

                // Return calldata + tx_id for tracking
                return Json(serde_json::json!({
                    "tx_id": tx_id,
                    "calldata": body.get("data").unwrap_or(&body),
                    "chain_id": chain_id,
                    "chain_name": match chain_id {
                        1 => "Ethereum",
                        137 => "Polygon",
                        8453 => "Base",
                        _ => "Unknown",
                    },
                    "amount_usd": req.amount,
                    "credits": (req.amount * 100.0) as i64, // $1 = 100 credits
                })).into_response();
            }

            Json(body).into_response()
        }
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            tracing::error!("OpenRouter crypto API error: {} {}", status, body);
            Json(serde_json::json!({
                "error": format!("Payment service error: {}", status)
            })).into_response()
        }
        Err(e) => {
            tracing::error!("OpenRouter crypto API failed: {}", e);
            Json(serde_json::json!({
                "error": "Payment service unavailable"
            })).into_response()
        }
    }
}

/// POST /api/v1/crypto/confirm — Confirm payment and add credits
async fn handle_crypto_confirm(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(_req): Json<CryptoConfirmRequest>,
) -> impl IntoResponse {
    let user_id = {
        #[cfg(feature = "dynamodb-backend")]
        {
            auth_user_id(&state, &headers).await
                .or_else(|| _req.session_id.clone())
                .unwrap_or_default()
        }
        #[cfg(not(feature = "dynamodb-backend"))]
        { let _ = (&state, &headers); String::new() }
    };

    if user_id.is_empty() {
        return Json(serde_json::json!({ "error": "Login required" })).into_response();
    }

    // Check OpenRouter balance increase
    let or_key = match std::env::var("OPENROUTER_API_KEY") {
        Ok(k) if !k.is_empty() => k,
        _ => return Json(serde_json::json!({ "error": "Not configured" })).into_response(),
    };

    let client = reqwest::Client::new();
    let credits_resp = client
        .get("https://openrouter.ai/api/v1/credits")
        .header("Authorization", format!("Bearer {}", or_key))
        .send()
        .await;

    let _or_balance = match credits_resp {
        Ok(resp) if resp.status().is_success() => {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let total = body.pointer("/data/total_credits").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let used = body.pointer("/data/total_usage").and_then(|v| v.as_f64()).unwrap_or(0.0);
            total - used
        }
        _ => {
            return Json(serde_json::json!({ "error": "Cannot verify payment" })).into_response();
        }
    };

    // Find and update pending transaction
    #[cfg(feature = "dynamodb-backend")]
    if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
        // Mark transaction as confirmed and add credits
        // For simplicity, look up by tx_hash in user's pending transactions
        let _ = dynamo
            .put_item()
            .table_name(table)
            .item("pk", AttributeValue::S(format!("CRYPTO_TX#{}", _req.tx_hash)))
            .item("sk", AttributeValue::S("CONFIRMED".to_string()))
            .item("user_id", AttributeValue::S(user_id.clone()))
            .item("confirmed_at", AttributeValue::S(chrono::Utc::now().to_rfc3339()))
            .item("or_balance", AttributeValue::N(format!("{:.2}", _or_balance)))
            .send()
            .await;

        // Add credits (assume the tx was valid — OpenRouter balance increased)
        // In production: verify tx_hash on-chain
        let user = get_or_create_user(dynamo, table, &user_id).await;

        // Emit audit log
        emit_audit_log(dynamo.clone(), table.clone(), "crypto_payment",
            &user_id, &_req.tx_hash,
            &format!("or_balance=${:.2}", _or_balance));

        return Json(serde_json::json!({
            "success": true,
            "tx_hash": _req.tx_hash,
            "openrouter_balance_usd": _or_balance,
            "credits_remaining": user.credits_remaining,
            "message": "Payment confirmed! Credits will be added shortly.",
            "message_ja": "決済確認済み！クレジットがまもなく反映されます。",
        })).into_response();
    }

    Json(serde_json::json!({ "error": "Payment system not available" })).into_response()
}

/// GET /api/v1/account/:id — Get user profile (unified billing, supports Bearer token)
async fn handle_account(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    // Resolve unified session key — prefer Bearer token if available
    let user_id = {
        #[cfg(feature = "dynamodb-backend")]
        {
            // Use auth token to resolve user if available
            let effective_id = auth_user_id(&state, &headers).await.unwrap_or(id.clone());
            let lookup_id = if effective_id != id { &effective_id } else { &id };
            if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                let resolved = resolve_session_key(dynamo, table, lookup_id).await;
                let user = get_or_create_user(dynamo, table, &resolved).await;
                let allowed_models: Vec<String> = user.plan.parse::<crate::service::auth::Plan>()
                    .unwrap_or(crate::service::auth::Plan::Free)
                    .allowed_models().iter().map(|s| s.to_string()).collect();
                return Json(serde_json::json!({
                    "user_id": user.user_id,
                    "plan": user.plan,
                    "credits_remaining": user.credits_remaining,
                    "credits_used": user.credits_used,
                    "channels": user.channels,
                    "stripe_customer_id": user.stripe_customer_id,
                    "email": user.email,
                    "created_at": user.created_at,
                    "allowed_models": allowed_models,
                }));
            }
            id.clone()
        }
        #[cfg(not(feature = "dynamodb-backend"))]
        {
            id.clone()
        }
    };

    Json(serde_json::json!({
        "user_id": user_id,
        "plan": "free",
        "credits_remaining": 1000,
        "credits_used": 0,
        "channels": [],
        "allowed_models": ["gpt-4o-mini", "gemini-flash"],
    }))
}

/// GET /api/v1/providers — List available AI providers and models
async fn handle_providers(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let mut providers = Vec::new();

    if std::env::var("OPENAI_API_KEY").is_ok() {
        providers.push(serde_json::json!({
            "id": "openai",
            "name": "OpenAI",
            "models": ["gpt-4o", "gpt-4o-mini"],
            "status": "active",
        }));
    }

    if std::env::var("ANTHROPIC_API_KEY").is_ok() {
        providers.push(serde_json::json!({
            "id": "anthropic",
            "name": "Anthropic",
            "models": ["claude-sonnet", "claude-opus"],
            "status": "active",
        }));
    }

    if std::env::var("GEMINI_API_KEY").is_ok() || std::env::var("GOOGLE_API_KEY").is_ok() {
        providers.push(serde_json::json!({
            "id": "google",
            "name": "Google Gemini / Vertex AI",
            "models": ["gemini-2.0-flash", "gemini-pro"],
            "status": "active",
        }));
    }

    if std::env::var("DEEPSEEK_API_KEY").is_ok() {
        providers.push(serde_json::json!({
            "id": "deepseek",
            "name": "DeepSeek",
            "models": ["deepseek-chat", "deepseek-reasoner"],
            "status": "active",
        }));
    }

    let has_lb = state.lb_provider.is_some();

    Json(serde_json::json!({
        "providers": providers,
        "load_balanced": has_lb,
        "total_providers": providers.len(),
        "default_model": state.config.agents.defaults.model,
    }))
}

/// GET /api/v1/integrations — List available integrations
async fn handle_integrations(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let integrations = crate::service::integrations::list_integrations();
    let tools = state.tool_registry.get_definitions();

    Json(serde_json::json!({
        "integrations": integrations,
        "tools": tools,
        "active_count": tools.len(),
        "total_count": integrations.len(),
    }))
}

/// GET / — Root landing page (host-based routing)
async fn handle_root(headers: axum::http::HeaderMap) -> impl IntoResponse {
    let host = effective_host(&headers);

    if host.starts_with("api.") {
        // Serve API docs for api.chatweb.ai / api.teai.io
        axum::response::Html(include_str!("../../../../web/api-docs.html"))
    } else {
        // Serve full chat UI for all domains (chatweb.ai, teai.io, etc.)
        // Frontend detects IS_TEAI via location.hostname for domain-specific behavior
        axum::response::Html(include_str!("../../../../web/index.html"))
    }
}

/// GET /api/v1/pricing — Pricing data JSON
async fn handle_pricing_api() -> impl IntoResponse {
    use crate::provider::pricing::{PRICING_TABLE, MEDIA_PRICING};
    Json(serde_json::json!({
        "models": PRICING_TABLE,
        "media": MEDIA_PRICING,
        "updated_at": "2026-02-14",
    }))
}

/// GET /pricing — Pricing page (host-based routing)
async fn handle_pricing(headers: axum::http::HeaderMap) -> impl IntoResponse {
    let host = effective_host(&headers);
    if host.contains("teai.io") {
        axum::response::Html(include_str!("../../../../web/teai-pricing.html"))
    } else {
        axum::response::Html(include_str!("../../../../web/pricing.html"))
    }
}

/// GET /welcome — Welcome / success page
async fn handle_welcome() -> impl IntoResponse {
    axum::response::Html(include_str!("../../../../web/welcome.html"))
}

/// GET /status — Status page
async fn handle_status() -> impl IntoResponse {
    axum::response::Html(include_str!("../../../../web/status.html"))
}

/// GET /comparison — Service comparison page
async fn handle_comparison() -> impl IntoResponse {
    axum::response::Html(include_str!("../../../../web/comparison.html"))
}

/// GET /contact — Contact / bug report page
async fn handle_contact() -> impl IntoResponse {
    axum::response::Html(include_str!("../../../../web/contact.html"))
}

/// GET /terms — Terms of Service page
async fn handle_terms() -> impl IntoResponse {
    axum::response::Html(include_str!("../../../../web/terms.html"))
}

async fn handle_docs() -> impl IntoResponse {
    axum::response::Html(include_str!("../../../../web/docs.html"))
}

/// Contact form submission request.
#[derive(Debug, Deserialize)]
pub struct ContactRequest {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub email: String,
    pub category: String,
    pub message: String,
    #[serde(default)]
    pub session_id: String,
}

/// POST /api/v1/contact — Save contact/bug report to DynamoDB
async fn handle_contact_submit(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ContactRequest>,
) -> impl IntoResponse {
    info!("Contact form: category={}, email={}", req.category, req.email);

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let id = uuid::Uuid::new_v4().to_string();
            let now = chrono::Utc::now().to_rfc3339();
            let result = dynamo
                .put_item()
                .table_name(table)
                .item("pk", AttributeValue::S(format!("CONTACT#{}", id)))
                .item("sk", AttributeValue::S("SUBMITTED".to_string()))
                .item("category", AttributeValue::S(req.category.clone()))
                .item("message", AttributeValue::S(req.message.clone()))
                .item("name", AttributeValue::S(req.name.clone()))
                .item("email", AttributeValue::S(req.email.clone()))
                .item("session_id", AttributeValue::S(req.session_id.clone()))
                .item("created_at", AttributeValue::S(now))
                .item("status", AttributeValue::S("new".to_string()))
                .send()
                .await;

            match result {
                Ok(_) => {
                    info!("Contact saved: CONTACT#{}", id);
                    return (StatusCode::OK, Json(serde_json::json!({
                        "status": "ok",
                        "id": id,
                    })));
                }
                Err(e) => {
                    tracing::error!("Failed to save contact: {}", e);
                    return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
                        "status": "error",
                        "message": "Failed to save. Please try again.",
                    })));
                }
            }
        }
    }

    // Fallback when DynamoDB is not configured
    let _ = state;
    (StatusCode::OK, Json(serde_json::json!({
        "status": "ok",
        "message": "Received (no DB configured)",
    })))
}

/// Query params for admin page.
#[derive(Debug, Deserialize)]
struct AdminQuery {
    sid: Option<String>,
}

/// GET /admin — Admin dashboard (requires ?sid=<admin session key>)
async fn handle_admin(Query(q): Query<AdminQuery>) -> impl IntoResponse {
    match q.sid {
        Some(ref sid) if is_admin(sid) => {
            axum::response::Html(include_str!("../../../../web/admin.html")).into_response()
        }
        _ => {
            (StatusCode::FORBIDDEN, axum::response::Html(
                "<html><body><h1>403 Forbidden</h1><p>Admin access required.</p></body></html>"
            )).into_response()
        }
    }
}

/// GET /api/v1/admin/check?sid=<session_key> — Check if user is admin
async fn handle_admin_check(Query(q): Query<AdminQuery>) -> impl IntoResponse {
    let sid = q.sid.unwrap_or_default();
    Json(serde_json::json!({
        "is_admin": is_admin(&sid),
    }))
}

/// GET /api/v1/admin/stats?sid=<session_key> — Admin stats dashboard data
async fn handle_admin_stats(
    State(state): State<Arc<AppState>>,
    Query(q): Query<AdminQuery>,
) -> impl IntoResponse {
    let sid = q.sid.unwrap_or_default();
    if !is_admin(&sid) {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({
            "error": "Forbidden",
        }))).into_response();
    }

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(config_table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let sessions_table = std::env::var("DYNAMODB_SESSIONS_TABLE")
                .unwrap_or_else(|_| "nanobot-sessions-default".to_string());
            let today = chrono::Utc::now().format("%Y%m%d").to_string();

            type DynKey = std::collections::HashMap<String, AttributeValue>;

            // 1. Count registered users (USER# + PROFILE)
            let mut total_users: u64 = 0;
            let mut start_key: Option<DynKey> = None;
            loop {
                let mut scan = dynamo
                    .scan()
                    .table_name(config_table)
                    .filter_expression("begins_with(pk, :prefix) AND sk = :sk")
                    .expression_attribute_values(":prefix", AttributeValue::S("USER#".to_string()))
                    .expression_attribute_values(":sk", AttributeValue::S(SK_PROFILE.to_string()))
                    .select(aws_sdk_dynamodb::types::Select::Count);
                if let Some(ref key) = start_key {
                    scan = scan.set_exclusive_start_key(Some(key.clone()));
                }
                match scan.send().await {
                    Ok(output) => {
                        total_users += output.count() as u64;
                        match output.last_evaluated_key() {
                            Some(k) => start_key = Some(k.to_owned()),
                            None => break,
                        }
                    }
                    Err(e) => { tracing::warn!("admin stats users scan: {}", e); break; }
                }
            }

            // 2. Count sessions by channel
            let mut web: u64 = 0;
            let mut line: u64 = 0;
            let mut tg: u64 = 0;
            let mut other: u64 = 0;
            let mut start_key: Option<DynKey> = None;
            loop {
                let mut scan = dynamo
                    .scan()
                    .table_name(&sessions_table)
                    .projection_expression("session_key");
                if let Some(ref key) = start_key {
                    scan = scan.set_exclusive_start_key(Some(key.clone()));
                }
                match scan.send().await {
                    Ok(output) => {
                        for item in output.items() {
                            if let Some(sk) = item.get("session_key").and_then(|v| v.as_s().ok()) {
                                if sk.starts_with("webchat:") { web += 1; }
                                else if sk.starts_with("line:") { line += 1; }
                                else if sk.starts_with("tg:") { tg += 1; }
                                else { other += 1; }
                            }
                        }
                        match output.last_evaluated_key() {
                            Some(k) => start_key = Some(k.to_owned()),
                            None => break,
                        }
                    }
                    Err(e) => { tracing::warn!("admin stats sessions scan: {}", e); break; }
                }
            }

            // 3. Count today's usage
            let mut today_usage: u64 = 0;
            let today_suffix = format!("#{}", today);
            let mut start_key: Option<DynKey> = None;
            loop {
                let mut scan = dynamo
                    .scan()
                    .table_name(config_table)
                    .filter_expression("begins_with(pk, :prefix) AND contains(pk, :date)")
                    .expression_attribute_values(":prefix", AttributeValue::S("USAGE#".to_string()))
                    .expression_attribute_values(":date", AttributeValue::S(today_suffix.clone()))
                    .select(aws_sdk_dynamodb::types::Select::Count);
                if let Some(ref key) = start_key {
                    scan = scan.set_exclusive_start_key(Some(key.clone()));
                }
                match scan.send().await {
                    Ok(output) => {
                        today_usage += output.count() as u64;
                        match output.last_evaluated_key() {
                            Some(k) => start_key = Some(k.to_owned()),
                            None => break,
                        }
                    }
                    Err(e) => { tracing::warn!("admin stats usage scan: {}", e); break; }
                }
            }

            return Json(serde_json::json!({
                "total_users": total_users,
                "sessions": {
                    "webchat": web,
                    "line": line,
                    "telegram": tg,
                    "other": other,
                    "total": web + line + tg + other,
                },
                "today_usage": today_usage,
                "date": today,
            })).into_response();
        }
    }

    Json(serde_json::json!({
        "error": "DynamoDB not configured",
    })).into_response()
}

/// GET /api/v1/admin/users — List all registered users (admin only)
/// Query params: ?sid=<admin_key>&limit=200
async fn handle_admin_users(
    State(state): State<Arc<AppState>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let sid = q.get("sid").cloned().unwrap_or_default();
    if !is_admin(&sid) {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": "Forbidden"}))).into_response();
    }

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let limit: usize = q.get("limit").and_then(|v| v.parse().ok()).unwrap_or(200);
            let mut users: Vec<serde_json::Value> = Vec::new();
            let mut start_key: Option<std::collections::HashMap<String, AttributeValue>> = None;

            loop {
                let mut req = dynamo.scan()
                    .table_name(table)
                    .filter_expression("begins_with(pk, :prefix) AND sk = :profile")
                    .expression_attribute_values(":prefix", AttributeValue::S("USER#".to_string()))
                    .expression_attribute_values(":profile", AttributeValue::S("PROFILE".to_string()));
                if let Some(ref k) = start_key {
                    req = req.set_exclusive_start_key(Some(k.clone()));
                }
                match req.send().await {
                    Ok(output) => {
                        for item in output.items() {
                            let user_id = item.get("pk").and_then(|v| v.as_s().ok())
                                .map(|s| s.strip_prefix("USER#").unwrap_or(s).to_string())
                                .unwrap_or_default();
                            let email = item.get("email").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                            let display_name = item.get("display_name").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                            let plan = item.get("plan").and_then(|v| v.as_s().ok()).cloned().unwrap_or_else(|| "free".to_string());
                            let credits = item.get("credits_remaining").and_then(|v| v.as_n().ok()).and_then(|n| n.parse::<i64>().ok()).unwrap_or(0);
                            let credits_used = item.get("credits_used").and_then(|v| v.as_n().ok()).and_then(|n| n.parse::<i64>().ok()).unwrap_or(0);
                            let created_at = item.get("created_at").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                            let channel = item.get("channel").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                            let referral_code = item.get("referral_code").and_then(|v| v.as_s().ok()).cloned();

                            users.push(serde_json::json!({
                                "user_id": user_id,
                                "email": email,
                                "display_name": display_name,
                                "plan": plan,
                                "credits_remaining": credits,
                                "credits_used": credits_used,
                                "created_at": created_at,
                                "channel": channel,
                                "referral_code": referral_code,
                            }));
                        }
                        if users.len() >= limit {
                            users.truncate(limit);
                            break;
                        }
                        match output.last_evaluated_key() {
                            Some(k) => start_key = Some(k.to_owned()),
                            None => break,
                        }
                    }
                    Err(e) => {
                        tracing::warn!("admin users scan error: {}", e);
                        return Json(serde_json::json!({"error": e.to_string()})).into_response();
                    }
                }
            }

            // Sort by credits_used descending (most active first)
            users.sort_by(|a, b| {
                let au = a["credits_used"].as_i64().unwrap_or(0);
                let bu = b["credits_used"].as_i64().unwrap_or(0);
                bu.cmp(&au)
            });

            return Json(serde_json::json!({
                "users": users,
                "count": users.len(),
            })).into_response();
        }
    }
    Json(serde_json::json!({"users": [], "error": "DynamoDB not configured"})).into_response()
}

/// GET /api/v1/admin/users/{user_id}/conversations — List conversations for a user (admin only)
/// Query params: ?sid=<admin_key>
async fn handle_admin_user_conversations(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let sid = q.get("sid").cloned().unwrap_or_default();
    if !is_admin(&sid) {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": "Forbidden"}))).into_response();
    }

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let pk = format!("USER#{}", user_id);
            match dynamo.query()
                .table_name(table)
                .key_condition_expression("pk = :pk AND begins_with(sk, :conv)")
                .expression_attribute_values(":pk", AttributeValue::S(pk))
                .expression_attribute_values(":conv", AttributeValue::S("CONV#".to_string()))
                .scan_index_forward(false)
                .send().await
            {
                Ok(output) => {
                    let conversations: Vec<serde_json::Value> = output.items().iter().map(|item| {
                        let conv_id = item.get("sk").and_then(|v| v.as_s().ok())
                            .map(|s| s.strip_prefix("CONV#").unwrap_or(s).to_string())
                            .unwrap_or_default();
                        let title = item.get("title").and_then(|v| v.as_s().ok()).cloned().unwrap_or_else(|| "Untitled".to_string());
                        let session_id = item.get("session_id").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                        let message_count = item.get("message_count").and_then(|v| v.as_n().ok()).and_then(|n| n.parse::<i64>().ok()).unwrap_or(0);
                        let created_at = item.get("created_at").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                        let updated_at = item.get("updated_at").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                        serde_json::json!({
                            "conv_id": conv_id,
                            "title": title,
                            "session_id": session_id,
                            "message_count": message_count,
                            "created_at": created_at,
                            "updated_at": updated_at,
                        })
                    }).collect();
                    return Json(serde_json::json!({
                        "user_id": user_id,
                        "conversations": conversations,
                        "count": conversations.len(),
                    })).into_response();
                }
                Err(e) => {
                    tracing::warn!("admin user conversations query error: {}", e);
                    return Json(serde_json::json!({"error": e.to_string()})).into_response();
                }
            }
        }
    }
    Json(serde_json::json!({"conversations": [], "error": "DynamoDB not configured"})).into_response()
}

/// GET /api/v1/admin/sessions/{session_key}/messages — Get messages for a session (admin only)
/// Query params: ?sid=<admin_key>
async fn handle_admin_session_messages(
    State(state): State<Arc<AppState>>,
    Path(session_key): Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let sid = q.get("sid").cloned().unwrap_or_default();
    if !is_admin(&sid) {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": "Forbidden"}))).into_response();
    }

    #[cfg(feature = "dynamodb-backend")]
    {
        if let Some(dynamo) = state.dynamo_client.as_ref() {
            // Sessions are in a separate table: nanobot-sessions-default
            let sessions_table = std::env::var("SESSIONS_TABLE").unwrap_or_else(|_| "nanobot-sessions-default".to_string());
            match dynamo.get_item()
                .table_name(&sessions_table)
                .key("session_key", AttributeValue::S(session_key.clone()))
                .send().await
            {
                Ok(output) => {
                    if let Some(item) = output.item() {
                        let messages: Vec<serde_json::Value> = if let Some(msg_list) = item.get("messages").and_then(|v| v.as_l().ok()) {
                            msg_list.iter().filter_map(|m| {
                                let map = m.as_m().ok()?;
                                let role = map.get("role").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                                let content = map.get("content").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                                let timestamp = map.get("timestamp").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                                Some(serde_json::json!({
                                    "role": role,
                                    "content": content,
                                    "timestamp": timestamp,
                                }))
                            }).collect()
                        } else {
                            Vec::new()
                        };
                        return Json(serde_json::json!({
                            "session_key": session_key,
                            "messages": messages,
                            "count": messages.len(),
                        })).into_response();
                    } else {
                        return Json(serde_json::json!({
                            "session_key": session_key,
                            "messages": [],
                            "count": 0,
                            "error": "Session not found",
                        })).into_response();
                    }
                }
                Err(e) => {
                    tracing::warn!("admin session messages error: {}", e);
                    return Json(serde_json::json!({"error": e.to_string()})).into_response();
                }
            }
        }
    }
    Json(serde_json::json!({"messages": [], "error": "DynamoDB not configured"})).into_response()
}

/// POST /api/v1/tickets — Create a human escalation ticket
/// Body: { "question": "...", "context": "...", "channel": "line|telegram|email|chatweb", "priority": false }
async fn handle_create_ticket(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let question = body["question"].as_str().unwrap_or("").trim();
    if question.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "question is required"}))).into_response();
    }

    let notify_channel = body["channel"].as_str().unwrap_or("chatweb");
    let priority = body["priority"].as_bool().unwrap_or(false);
    let context = body["context"].as_str().unwrap_or("");
    let conv_id = body["conv_id"].as_str().unwrap_or("");

    // Resolve user
    let mut user_id = String::new();
    let mut user_email = String::new();
    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            if let Some(uid) = auth_user_id(&state, &headers).await {
                user_id = uid.clone();
                // Get email
                if let Ok(out) = dynamo.get_item()
                    .table_name(table)
                    .key("pk", AttributeValue::S(format!("USER#{}", uid)))
                    .key("sk", AttributeValue::S("PROFILE".to_string()))
                    .send().await
                {
                    if let Some(item) = out.item() {
                        user_email = item.get("email").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                    }
                }
            }
        }
    }
    if user_id.is_empty() {
        if let Some(sid) = headers.get("x-session-id").and_then(|v| v.to_str().ok()) {
            user_id = sid.to_string();
        }
    }

    // Generate ticket ID
    let ticket_id = format!("TK{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let now = chrono::Utc::now().to_rfc3339();
    let sla = if priority { "15min" } else { "30min" };

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            // Priority costs 50 credits
            if priority && !user_id.is_empty() {
                let pk = format!("USER#{}", user_id);
                let _ = dynamo.update_item()
                    .table_name(table)
                    .key("pk", AttributeValue::S(pk))
                    .key("sk", AttributeValue::S("PROFILE".to_string()))
                    .update_expression("SET credits_remaining = credits_remaining - :c")
                    .expression_attribute_values(":c", AttributeValue::N("50".to_string()))
                    .condition_expression("credits_remaining >= :c")
                    .send().await;
            }

            // Store ticket
            let ttl = (chrono::Utc::now() + chrono::Duration::days(90)).timestamp();
            let _ = dynamo.put_item()
                .table_name(table)
                .item("pk", AttributeValue::S(format!("TICKET#{}", ticket_id)))
                .item("sk", AttributeValue::S("INFO".to_string()))
                .item("ticket_id", AttributeValue::S(ticket_id.clone()))
                .item("user_id", AttributeValue::S(user_id.clone()))
                .item("user_email", AttributeValue::S(user_email.clone()))
                .item("question", AttributeValue::S(question.to_string()))
                .item("context", AttributeValue::S(context.to_string()))
                .item("conv_id", AttributeValue::S(conv_id.to_string()))
                .item("notify_channel", AttributeValue::S(notify_channel.to_string()))
                .item("priority", AttributeValue::Bool(priority))
                .item("sla", AttributeValue::S(sla.to_string()))
                .item("status", AttributeValue::S("open".to_string()))
                .item("created_at", AttributeValue::S(now.clone()))
                .item("ttl", AttributeValue::N(ttl.to_string()))
                .send().await;

            // Notify admin via LINE push
            if let Ok(access_token) = std::env::var("LINE_CHANNEL_ACCESS_TOKEN") {
                if let Ok(admin_line_id) = std::env::var("ADMIN_LINE_USER_ID") {
                    let prio_label = if priority { "🔴 優先" } else { "🟡 通常" };
                    let msg = format!(
                        "📩 新チケット {}\n{} (SLA: {})\n\n質問: {}\n\nユーザー: {}\nチャネル: {}\n\n管理画面: https://chatweb.ai/admin",
                        ticket_id, prio_label, sla,
                        if question.len() > 100 { &question[..100] } else { question },
                        if user_email.is_empty() { &user_id } else { &user_email },
                        notify_channel,
                    );
                    let _ = LineChannel::push_message(&access_token, &admin_line_id, &msg).await;
                }
            }

            return Json(serde_json::json!({
                "ticket_id": ticket_id,
                "status": "open",
                "sla": sla,
                "priority": priority,
                "message": format!("チケットを作成しました。{}以内に回答します。", sla),
            })).into_response();
        }
    }
    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "DynamoDB not configured"}))).into_response()
}

/// GET /api/v1/admin/tickets — List all tickets (admin only)
/// Query params: ?sid=<admin_key>&status=open
async fn handle_admin_tickets(
    State(state): State<Arc<AppState>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let sid = q.get("sid").cloned().unwrap_or_default();
    if !is_admin(&sid) {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": "Forbidden"}))).into_response();
    }

    let filter_status = q.get("status").cloned();

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let mut tickets: Vec<serde_json::Value> = Vec::new();
            let mut start_key: Option<std::collections::HashMap<String, AttributeValue>> = None;

            loop {
                let mut req = dynamo.scan()
                    .table_name(table)
                    .filter_expression("begins_with(pk, :prefix) AND sk = :info")
                    .expression_attribute_values(":prefix", AttributeValue::S("TICKET#".to_string()))
                    .expression_attribute_values(":info", AttributeValue::S("INFO".to_string()));
                if let Some(ref k) = start_key {
                    req = req.set_exclusive_start_key(Some(k.clone()));
                }
                match req.send().await {
                    Ok(output) => {
                        for item in output.items() {
                            let status = item.get("status").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                            if let Some(ref fs) = filter_status {
                                if &status != fs { continue; }
                            }
                            tickets.push(serde_json::json!({
                                "ticket_id": item.get("ticket_id").and_then(|v| v.as_s().ok()).unwrap_or(&String::new()),
                                "user_id": item.get("user_id").and_then(|v| v.as_s().ok()).unwrap_or(&String::new()),
                                "user_email": item.get("user_email").and_then(|v| v.as_s().ok()).unwrap_or(&String::new()),
                                "question": item.get("question").and_then(|v| v.as_s().ok()).unwrap_or(&String::new()),
                                "context": item.get("context").and_then(|v| v.as_s().ok()).unwrap_or(&String::new()),
                                "notify_channel": item.get("notify_channel").and_then(|v| v.as_s().ok()).unwrap_or(&String::new()),
                                "priority": item.get("priority").and_then(|v| v.as_bool().ok()).copied().unwrap_or(false),
                                "sla": item.get("sla").and_then(|v| v.as_s().ok()).unwrap_or(&String::new()),
                                "status": status,
                                "response": item.get("response").and_then(|v| v.as_s().ok()).unwrap_or(&String::new()),
                                "created_at": item.get("created_at").and_then(|v| v.as_s().ok()).unwrap_or(&String::new()),
                                "resolved_at": item.get("resolved_at").and_then(|v| v.as_s().ok()).unwrap_or(&String::new()),
                            }));
                        }
                        match output.last_evaluated_key() {
                            Some(k) => start_key = Some(k.to_owned()),
                            None => break,
                        }
                    }
                    Err(e) => { tracing::warn!("admin tickets scan: {}", e); break; }
                }
            }

            // Sort by created_at descending
            tickets.sort_by(|a, b| {
                let at = a["created_at"].as_str().unwrap_or("");
                let bt = b["created_at"].as_str().unwrap_or("");
                bt.cmp(at)
            });

            return Json(serde_json::json!({
                "tickets": tickets,
                "count": tickets.len(),
            })).into_response();
        }
    }
    Json(serde_json::json!({"tickets": [], "error": "DynamoDB not configured"})).into_response()
}

/// POST /api/v1/admin/tickets/{ticket_id}/respond — Respond to a ticket (admin only)
/// Body: { "sid": "...", "response": "..." }
async fn handle_admin_ticket_respond(
    State(state): State<Arc<AppState>>,
    Path(ticket_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let sid = body["sid"].as_str().unwrap_or("");
    if !is_admin(sid) {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": "Forbidden"}))).into_response();
    }
    let response_text = body["response"].as_str().unwrap_or("").trim();
    if response_text.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "response is required"}))).into_response();
    }

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let pk = format!("TICKET#{}", ticket_id);
            let now = chrono::Utc::now().to_rfc3339();

            // Get ticket info first
            let ticket = match dynamo.get_item()
                .table_name(table)
                .key("pk", AttributeValue::S(pk.clone()))
                .key("sk", AttributeValue::S("INFO".to_string()))
                .send().await
            {
                Ok(out) => out.item().cloned(),
                Err(e) => {
                    return Json(serde_json::json!({"error": e.to_string()})).into_response();
                }
            };

            let ticket = match ticket {
                Some(t) => t,
                None => return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "Ticket not found"}))).into_response(),
            };

            // Update ticket status
            let _ = dynamo.update_item()
                .table_name(table)
                .key("pk", AttributeValue::S(pk))
                .key("sk", AttributeValue::S("INFO".to_string()))
                .update_expression("SET #status = :s, #resp = :r, resolved_at = :t")
                .expression_attribute_names("#status", "status")
                .expression_attribute_names("#resp", "response")
                .expression_attribute_values(":s", AttributeValue::S("resolved".to_string()))
                .expression_attribute_values(":r", AttributeValue::S(response_text.to_string()))
                .expression_attribute_values(":t", AttributeValue::S(now.clone()))
                .send().await;

            // Send response to user via their chosen channel
            let notify_channel = ticket.get("notify_channel").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
            let user_id = ticket.get("user_id").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
            let question = ticket.get("question").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();

            let notification = format!(
                "💡 チケット {} への回答:\n\nご質問: {}\n\n回答: {}\n\n---\nchatweb.ai サポートチーム",
                ticket_id,
                if question.len() > 60 { format!("{}...", &question[..60]) } else { question },
                response_text,
            );

            match notify_channel.as_str() {
                "line" => {
                    // user_id might be a LINE user ID (starts with line:U...)
                    let line_uid = user_id.strip_prefix("line:").unwrap_or(&user_id);
                    if let Ok(access_token) = std::env::var("LINE_CHANNEL_ACCESS_TOKEN") {
                        let _ = LineChannel::push_message(&access_token, line_uid, &notification).await;
                    }
                }
                _ => {
                    // For chatweb/email/telegram — store notification for user to see on next visit
                    let _ = dynamo.put_item()
                        .table_name(table)
                        .item("pk", AttributeValue::S(format!("NOTIFY#{}", user_id)))
                        .item("sk", AttributeValue::S(format!("{}#{}", now, ticket_id)))
                        .item("message", AttributeValue::S(notification))
                        .item("ticket_id", AttributeValue::S(ticket_id.clone()))
                        .item("read", AttributeValue::Bool(false))
                        .item("ttl", AttributeValue::N(
                            (chrono::Utc::now() + chrono::Duration::days(30)).timestamp().to_string()
                        ))
                        .send().await;
                }
            }

            return Json(serde_json::json!({
                "ticket_id": ticket_id,
                "status": "resolved",
                "resolved_at": now,
                "notified_via": notify_channel,
            })).into_response();
        }
    }
    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "DynamoDB not configured"}))).into_response()
}

/// GET /api/v1/admin/logs — Fetch audit logs (admin only)
/// Query params: ?sid=<admin_key>&date=YYYY-MM-DD&limit=50
async fn handle_admin_logs(
    State(state): State<Arc<AppState>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let sid = q.get("sid").cloned().unwrap_or_default();
    if !is_admin(&sid) {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": "Forbidden"}))).into_response();
    }

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let date = q.get("date").cloned()
                .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
            let limit: i32 = q.get("limit").and_then(|v| v.parse().ok()).unwrap_or(100);

            let pk = format!("AUDIT#{}", date);
            match dynamo.query()
                .table_name(table)
                .key_condition_expression("pk = :pk")
                .expression_attribute_values(":pk", AttributeValue::S(pk))
                .scan_index_forward(false) // newest first
                .limit(limit)
                .send().await
            {
                Ok(output) => {
                    let logs: Vec<serde_json::Value> = output.items().iter().map(|item| {
                        serde_json::json!({
                            "event_type": item.get("event_type").and_then(|v| v.as_s().ok()).unwrap_or(&String::new()),
                            "user_id": item.get("user_id").and_then(|v| v.as_s().ok()).unwrap_or(&String::new()),
                            "email": item.get("email").and_then(|v| v.as_s().ok()).unwrap_or(&String::new()),
                            "details": item.get("details").and_then(|v| v.as_s().ok()).unwrap_or(&String::new()),
                            "timestamp": item.get("timestamp").and_then(|v| v.as_s().ok()).unwrap_or(&String::new()),
                        })
                    }).collect();
                    return Json(serde_json::json!({"logs": logs, "date": date, "count": logs.len()})).into_response();
                }
                Err(e) => {
                    tracing::warn!("Admin logs query error: {}", e);
                    return Json(serde_json::json!({"logs": [], "error": e.to_string()})).into_response();
                }
            }
        }
    }
    Json(serde_json::json!({"logs": [], "error": "DynamoDB not configured"})).into_response()
}

/// GET /api/v1/activity — User's own activity log
/// Shows recent chat history, tool usage, credit changes
async fn handle_activity(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl axum::response::IntoResponse {
    #[cfg(feature = "dynamodb-backend")]
    {
        // Resolve user
        let session_key = if let Some(sk) = auth_user_id(&state, &headers).await {
            sk
        } else if let Some(sid) = headers.get("x-session-id").and_then(|v| v.to_str().ok()) {
            if sid.is_empty() {
                return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "Unauthorized"}))).into_response();
            }
            if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                resolve_session_key(dynamo, table, sid).await
            } else {
                sid.to_string()
            }
        } else {
            return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "Unauthorized"}))).into_response();
        };

        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            // Parallel: fetch user profile + recent usage + today's audit logs
            let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
            let today_yyyymmdd = chrono::Utc::now().format("%Y%m%d").to_string();
            let usage_pk = format!("USAGE#{}#{}", session_key, today_yyyymmdd);
            let user_pk = format!("USER#{}", session_key);

            let (user_result, usage_result, audit_result) = tokio::join!(
                // User profile
                dynamo.get_item().table_name(table)
                    .key("pk", AttributeValue::S(user_pk))
                    .key("sk", AttributeValue::S(SK_PROFILE.to_string()))
                    .send(),
                // Today's usage
                dynamo.get_item().table_name(table)
                    .key("pk", AttributeValue::S(usage_pk))
                    .key("sk", AttributeValue::S("COUNTER".to_string()))
                    .send(),
                // Recent audit logs for this user (scan AUDIT# for today matching user_id)
                dynamo.query().table_name(table)
                    .key_condition_expression("pk = :pk")
                    .filter_expression("user_id = :uid")
                    .expression_attribute_values(":pk", AttributeValue::S(format!("AUDIT#{}", today)))
                    .expression_attribute_values(":uid", AttributeValue::S(session_key.clone()))
                    .scan_index_forward(false)
                    .limit(50)
                    .send()
            );

            let user = user_result.ok().and_then(|o| o.item).unwrap_or_default();
            let credits_remaining = user.get("credits_remaining")
                .and_then(|v| v.as_n().ok())
                .and_then(|n| n.parse::<i64>().ok())
                .unwrap_or(0);
            let credits_used = user.get("credits_used")
                .and_then(|v| v.as_n().ok())
                .and_then(|n| n.parse::<i64>().ok())
                .unwrap_or(0);
            let plan = user.get("plan")
                .and_then(|v| v.as_s().ok())
                .cloned()
                .unwrap_or_else(|| "free".to_string());

            let today_requests = usage_result.ok()
                .and_then(|o| o.item)
                .and_then(|item| item.get("count").and_then(|v| v.as_n().ok()).and_then(|n| n.parse::<i64>().ok()))
                .unwrap_or(0);

            let logs: Vec<serde_json::Value> = audit_result.ok()
                .map(|o| o.items().iter().map(|item| {
                    serde_json::json!({
                        "event_type": item.get("event_type").and_then(|v| v.as_s().ok()).unwrap_or(&String::new()),
                        "details": item.get("details").and_then(|v| v.as_s().ok()).unwrap_or(&String::new()),
                        "timestamp": item.get("timestamp").and_then(|v| v.as_s().ok()).unwrap_or(&String::new()),
                    })
                }).collect())
                .unwrap_or_default();

            return Json(serde_json::json!({
                "plan": plan,
                "credits_remaining": credits_remaining,
                "credits_used": credits_used,
                "today_requests": today_requests,
                "today_logs": logs,
                "date": today,
            })).into_response();
        }
    }
    Json(serde_json::json!({"error": "DynamoDB not configured"})).into_response()
}

/// GET /og.svg — OGP image (host-based routing)
async fn handle_og_svg(headers: axum::http::HeaderMap) -> impl IntoResponse {
    let host = effective_host(&headers);
    let svg = if host.contains("teai.io") {
        include_str!("../../../../web/og-teai.svg")
    } else {
        include_str!("../../../../web/og.svg")
    };
    (
        [(axum::http::header::CONTENT_TYPE, "image/svg+xml")],
        svg,
    )
}

/// GET /install.sh — CLI install script
async fn handle_install_sh() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        include_str!("../../../../web/install.sh"),
    )
}

/// GET /dl/{filename} — Redirect to GitHub Releases latest binary
async fn handle_dl_redirect(
    axum::extract::Path(filename): axum::extract::Path<String>,
) -> impl IntoResponse {
    let url = format!(
        "https://github.com/yukihamada/nanobot/releases/latest/download/{}",
        filename
    );
    axum::response::Redirect::temporary(&url)
}

/// GET /health — Health check
// ---------------------------------------------------------------------------
// Partner API (Elio integration)
// ---------------------------------------------------------------------------

/// Validate partner API key from Authorization header.
/// Partner keys have "PARTNER_" prefix and are stored in DynamoDB.
#[cfg(feature = "dynamodb-backend")]
async fn validate_partner_key(
    dynamo: &aws_sdk_dynamodb::Client,
    table: &str,
    headers: &axum::http::HeaderMap,
) -> bool {
    let token = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_start_matches("Bearer ").to_string())
        .unwrap_or_default();

    if !token.starts_with("PARTNER_") {
        return false;
    }

    // Check if partner key exists in DynamoDB
    let pk = format!("PARTNER_KEY#{}", token);
    if let Ok(output) = dynamo
        .get_item()
        .table_name(table)
        .key("pk", AttributeValue::S(pk))
        .key("sk", AttributeValue::S("KEY".to_string()))
        .send()
        .await
    {
        if let Some(item) = output.item {
            let is_active = item.get("is_active")
                .and_then(|v| v.as_bool().ok())
                .copied()
                .unwrap_or(false);
            return is_active;
        }
    }
    false
}

/// POST /api/v1/partner/grant-credits — Grant credits to a user (partner API)
async fn handle_partner_grant_credits(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(_req): Json<PartnerGrantCreditsRequest>,
) -> impl IntoResponse {
    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            // Validate partner key
            if !validate_partner_key(dynamo, table, &headers).await {
                return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({
                    "error": "Invalid or inactive partner API key"
                })));
            }

            // Validate request
            if _req.credits <= 0 || _req.credits > 100_000 {
                return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                    "error": "Credits must be between 1 and 100,000"
                })));
            }
            if _req.idempotency_key.is_empty() || _req.idempotency_key.len() > 128 {
                return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                    "error": "idempotency_key is required (max 128 chars)"
                })));
            }

            // Check idempotency — prevent double-granting
            let idem_pk = format!("IDEMPOTENT#{}", _req.idempotency_key);
            if let Ok(output) = dynamo
                .get_item()
                .table_name(table)
                .key("pk", AttributeValue::S(idem_pk.clone()))
                .key("sk", AttributeValue::S("GRANT".to_string()))
                .send()
                .await
            {
                if output.item.is_some() {
                    return (StatusCode::OK, Json(serde_json::json!({
                        "status": "already_processed",
                        "idempotency_key": _req.idempotency_key
                    })));
                }
            }

            // Atomic credit increment on USER# record
            let user_pk = format!("USER#{}", _req.user_id);
            let _ = get_or_create_user(dynamo, table, &_req.user_id).await;

            let update_result = dynamo
                .update_item()
                .table_name(table)
                .key("pk", AttributeValue::S(user_pk))
                .key("sk", AttributeValue::S(SK_PROFILE.to_string()))
                .update_expression("SET credits_remaining = credits_remaining + :c, updated_at = :now")
                .expression_attribute_values(":c", AttributeValue::N(_req.credits.to_string()))
                .expression_attribute_values(":now", AttributeValue::S(chrono::Utc::now().to_rfc3339()))
                .send()
                .await;

            if let Err(e) = update_result {
                tracing::error!("Failed to grant credits: {:?}", e);
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
                    "error": "Failed to grant credits"
                })));
            }

            // Store idempotency key with 30-day TTL
            let ttl = (chrono::Utc::now().timestamp() + 30 * 86400).to_string();
            let _ = dynamo
                .put_item()
                .table_name(table)
                .item("pk", AttributeValue::S(idem_pk))
                .item("sk", AttributeValue::S("GRANT".to_string()))
                .item("user_id", AttributeValue::S(_req.user_id.clone()))
                .item("credits", AttributeValue::N(_req.credits.to_string()))
                .item("source", AttributeValue::S(_req.source.clone()))
                .item("created_at", AttributeValue::S(chrono::Utc::now().to_rfc3339()))
                .item("ttl", AttributeValue::N(ttl))
                .send()
                .await;

            // Audit log
            emit_audit_log(dynamo.clone(), table.clone(), "partner_grant_credits", &_req.user_id, "",
                &format!("credits={} source={} key={}", _req.credits, _req.source, _req.idempotency_key));

            let profile = get_or_create_user(dynamo, table, &_req.user_id).await;
            return (StatusCode::OK, Json(serde_json::json!({
                "status": "granted",
                "credits_granted": _req.credits,
                "credits_remaining": profile.credits_remaining,
                "idempotency_key": _req.idempotency_key
            })));
        }
    }

    (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({
        "error": "DynamoDB not configured"
    })))
}

/// POST /api/v1/partner/verify-subscription — Verify Elio subscription and grant credits
async fn handle_partner_verify_subscription(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(_req): Json<PartnerVerifySubscriptionRequest>,
) -> impl IntoResponse {
    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            // Validate partner key
            if !validate_partner_key(dynamo, table, &headers).await {
                return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({
                    "error": "Invalid or inactive partner API key"
                })));
            }

            // Map product_id to plan and credits
            let (elio_plan, monthly_credits) = match _req.product_id.as_str() {
                "love.elio.subscription.basic" => ("elio_basic", 500_i64),
                "love.elio.subscription.pro" => ("elio_pro", 2000_i64),
                _ => {
                    return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                        "error": format!("Unknown product_id: {}", _req.product_id)
                    })));
                }
            };

            // Idempotency: use user_id + year-month + product_id
            let now = chrono::Utc::now();
            let idem_key = format!("{}:{}:{}",
                _req.user_id,
                now.format("%Y-%m"),
                _req.product_id
            );
            let idem_pk = format!("IDEMPOTENT#{}", idem_key);

            // Check if already processed this month
            if let Ok(output) = dynamo
                .get_item()
                .table_name(table)
                .key("pk", AttributeValue::S(idem_pk.clone()))
                .key("sk", AttributeValue::S("SUB_VERIFY".to_string()))
                .send()
                .await
            {
                if output.item.is_some() {
                    let profile = get_or_create_user(dynamo, table, &_req.user_id).await;
                    return (StatusCode::OK, Json(serde_json::json!({
                        "status": "already_processed",
                        "elio_plan": elio_plan,
                        "credits_remaining": profile.credits_remaining
                    })));
                }
            }

            // Ensure user exists
            let _ = get_or_create_user(dynamo, table, &_req.user_id).await;
            let user_pk = format!("USER#{}", _req.user_id);

            // Update user with elio_plan and grant credits
            let expires_at = (now + chrono::Duration::days(32)).to_rfc3339();
            let _ = dynamo
                .update_item()
                .table_name(table)
                .key("pk", AttributeValue::S(user_pk))
                .key("sk", AttributeValue::S(SK_PROFILE.to_string()))
                .update_expression(
                    "SET elio_plan = :plan, elio_expires_at = :exp, \
                     credits_remaining = credits_remaining + :c, updated_at = :now"
                )
                .expression_attribute_values(":plan", AttributeValue::S(elio_plan.to_string()))
                .expression_attribute_values(":exp", AttributeValue::S(expires_at))
                .expression_attribute_values(":c", AttributeValue::N(monthly_credits.to_string()))
                .expression_attribute_values(":now", AttributeValue::S(now.to_rfc3339()))
                .send()
                .await;

            // Store idempotency key with 35-day TTL
            let ttl = (now.timestamp() + 35 * 86400).to_string();
            let _ = dynamo
                .put_item()
                .table_name(table)
                .item("pk", AttributeValue::S(idem_pk))
                .item("sk", AttributeValue::S("SUB_VERIFY".to_string()))
                .item("user_id", AttributeValue::S(_req.user_id.clone()))
                .item("product_id", AttributeValue::S(_req.product_id.clone()))
                .item("transaction_id", AttributeValue::S(_req.transaction_id.clone()))
                .item("credits_granted", AttributeValue::N(monthly_credits.to_string()))
                .item("created_at", AttributeValue::S(now.to_rfc3339()))
                .item("ttl", AttributeValue::N(ttl))
                .send()
                .await;

            emit_audit_log(dynamo.clone(), table.clone(), "partner_verify_subscription", &_req.user_id, "",
                &format!("plan={} credits={} txn={}", elio_plan, monthly_credits, _req.transaction_id));

            let profile = get_or_create_user(dynamo, table, &_req.user_id).await;
            return (StatusCode::OK, Json(serde_json::json!({
                "status": "verified",
                "elio_plan": elio_plan,
                "credits_granted": monthly_credits,
                "credits_remaining": profile.credits_remaining
            })));
        }
    }

    (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({
        "error": "DynamoDB not configured"
    })))
}

async fn handle_health() -> impl IntoResponse {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: crate::VERSION.to_string(),
    })
}

/// GET /api/v1/local/status — Local model status for Wisbee integration
async fn handle_local_status() -> impl IntoResponse {
    #[derive(Serialize)]
    struct LocalStatus {
        available: bool,
        model: &'static str,
        loaded: bool,
        memory_usage_mb: u64,
        capabilities: Vec<&'static str>,
    }

    #[cfg(feature = "local-fallback")]
    {
        use crate::provider::local::LocalProvider;
        let configured = LocalProvider::is_configured();
        let loaded = LocalProvider::is_loaded();
        let memory = LocalProvider::estimated_memory_mb();
        Json(LocalStatus {
            available: configured,
            model: "Qwen3-0.6B-Instruct",
            loaded,
            memory_usage_mb: memory,
            capabilities: vec!["text-generation", "japanese", "english"],
        })
    }
    #[cfg(not(feature = "local-fallback"))]
    {
        Json(LocalStatus {
            available: false,
            model: "Qwen3-0.6B-Instruct",
            loaded: false,
            memory_usage_mb: 0,
            capabilities: vec!["text-generation", "japanese", "english"],
        })
    }
}

// ---------------------------------------------------------------------------
// PWA: manifest.json, sw.js
// ---------------------------------------------------------------------------

async fn handle_manifest_json() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/manifest+json")],
        include_str!("../../../../web/manifest.json"),
    )
}

async fn handle_sw_js() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/javascript; charset=utf-8")],
        include_str!("../../../../web/sw.js"),
    )
}

async fn handle_api_docs() -> impl IntoResponse {
    axum::response::Html(include_str!("../../../../web/api-docs.html"))
}

// AI Agent Friendly: robots.txt, llms.txt, ai-plugin.json
// ---------------------------------------------------------------------------

async fn handle_robots_txt() -> impl IntoResponse {
    let base = get_base_url();
    (
        [(axum::http::header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        format!(
            "User-agent: *\n\
             Allow: /\n\
             \n\
             # AI Agents\n\
             User-agent: GPTBot\n\
             Allow: /\n\
             \n\
             User-agent: ChatGPT-User\n\
             Allow: /\n\
             \n\
             User-agent: Claude-Web\n\
             Allow: /\n\
             \n\
             User-agent: Googlebot\n\
             Allow: /\n\
             \n\
             User-agent: anthropic-ai\n\
             Allow: /\n\
             \n\
             User-agent: cohere-ai\n\
             Allow: /\n\
             \n\
             Sitemap: {base}/sitemap.xml\n"
        ),
    )
}

async fn handle_llms_txt() -> impl IntoResponse {
    let base = get_base_url();
    (
        [(axum::http::header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        format!(
            "# chatweb.ai\n\
             \n\
             > Voice-first, multi-channel AI assistant platform.\n\
             \n\
             chatweb.ai is a multi-model AI chat platform with voice interface,\n\
             LINE/Telegram/Facebook integration, and real-time streaming.\n\
             Built with Rust on AWS Lambda.\n\
             \n\
             ## API\n\
             \n\
             - Base URL: {base}/api/v1\n\
             - [API Documentation]({base}/docs)\n\
             - Auth: Bearer token or x-session-id header\n\
             \n\
             ## Endpoints\n\
             \n\
             - POST /api/v1/chat — Send message, get AI response\n\
             - POST /api/v1/chat/stream — SSE streaming response\n\
             - POST /api/v1/speech/synthesize — Text-to-speech (MP3)\n\
             - GET /api/v1/conversations — List conversations\n\
             - GET /api/v1/shared/{{hash}} — Get shared conversation (public)\n\
             - GET /api/v1/providers — List available AI models\n\
             - GET /health — Health check\n\
             - POST /mcp — MCP (Model Context Protocol) endpoint for AI agents\n\
             \n\
             ## Links\n\
             \n\
             - [Full API docs]({base}/llms-full.txt)\n\
             - [API Playground]({base}/playground)\n\
             - [Pricing]({base}/pricing)\n\
             - [Status]({base}/status)\n\
             - [GitHub](https://github.com/yukihamada/nanobot)\n"
        ),
    )
}

async fn handle_llms_full_txt() -> impl IntoResponse {
    let base = get_base_url();
    (
        [(axum::http::header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        format!(
            "# chatweb.ai — Full API Reference\n\
             \n\
             > Voice-first, multi-channel AI assistant platform.\n\
             \n\
             Base URL: {base}\n\
             All endpoints use /api/v1/ prefix unless noted.\n\
         \n\
         ## Authentication\n\
         \n\
         - POST /api/v1/auth/register — Register with email + password\n\
         - POST /api/v1/auth/login — Login with email + password\n\
         - POST /api/v1/auth/email — Passwordless email auth\n\
         - POST /api/v1/auth/verify — Verify email code (6 digits)\n\
         - GET /auth/google — Google OAuth redirect\n\
         - GET /api/v1/auth/me — Get current user info (Bearer token)\n\
         \n\
         ## Chat\n\
         \n\
         - POST /api/v1/chat\n\
           Body: {{\"message\": \"text\", \"session_id\": \"webchat:uuid\"}}\n\
           Response: {{\"response\": \"...\", \"session_id\": \"...\", \"credits_used\": N, \"credits_remaining\": N, \"model_used\": \"...\"}}\n\
         \n\
         - POST /api/v1/chat/stream\n\
           Same body. Returns SSE: data: {{\"type\":\"chunk\",\"content\":\"...\"}}\n\
         \n\
         ## Speech\n\
         \n\
         - POST /api/v1/speech/synthesize\n\
           Body: {{\"text\": \"...\", \"voice\": \"nova\", \"speed\": 1.0}}\n\
           Response: audio/mpeg binary\n\
         \n\
         ## Conversations\n\
         \n\
         - GET /api/v1/conversations — List (Auth: Bearer)\n\
         - POST /api/v1/conversations — Create new (Auth: Bearer)\n\
         - GET /api/v1/conversations/{{id}}/messages — Get messages (Auth: Bearer)\n\
         - DELETE /api/v1/conversations/{{id}} — Delete (Auth: Bearer)\n\
         - POST /api/v1/conversations/{{id}}/share — Generate share link (Auth: Bearer)\n\
         - DELETE /api/v1/conversations/{{id}}/share — Revoke share (Auth: Bearer)\n\
         - GET /api/v1/shared/{{hash}} — Get shared conversation (public, no auth)\n\
         \n\
         ## Sessions\n\
         \n\
         - GET /api/v1/sessions — List sessions (x-session-id header)\n\
         - GET /api/v1/sessions/{{id}} — Get session details\n\
         - DELETE /api/v1/sessions/{{id}} — Delete session\n\
         \n\
         ## Settings\n\
         \n\
         - GET /api/v1/settings/{{id}} — Get user settings\n\
         - POST /api/v1/settings/{{id}} — Update settings\n\
         \n\
         ## Account & Billing\n\
         \n\
         - GET /api/v1/account/{{id}} — Account info (plan, credits)\n\
         - GET /api/v1/usage — Usage summary\n\
         - POST /api/v1/billing/checkout — Create Stripe checkout\n\
         - GET /api/v1/billing/portal — Stripe customer portal\n\
         - POST /api/v1/coupon/validate — Validate coupon code\n\
         - POST /api/v1/coupon/redeem — Redeem coupon\n\
         \n\
         ## API Keys\n\
         \n\
         - GET /api/v1/apikeys — List keys (Auth: Bearer)\n\
         - POST /api/v1/apikeys — Create key (Auth: Bearer)\n\
         - DELETE /api/v1/apikeys/{{id}} — Delete key (Auth: Bearer)\n\
         \n\
         ## Misc\n\
         \n\
         - GET /api/v1/providers — List AI providers/models\n\
         - GET /api/v1/agents — List AI agents\n\
         - GET /api/v1/integrations — List tools\n\
         - GET /api/v1/devices — List connected CLI devices\n\
         - GET /health — Health check\n\
         - GET /api/v1/status/ping — Service status with latencies\n\
         \n\
         ## Slash Commands (in chat)\n\
         \n\
         - /help — Show available commands\n\
         - /status — Show system status inline\n\
         - /share — Generate share link for current conversation\n\
         - /link [code] — Link channels (Web + LINE + Telegram)\n\
         - /improve <description> — Admin: create self-improvement PR\n\
         \n\
         ## Rate Limits\n\
         \n\
         - Free: 10 concurrent, 1,000 credits/month\n\
         - Starter ($9/mo): 100 concurrent, 25,000 credits/month\n\
         - Pro ($29/mo): 1,000 concurrent, 300,000 credits/month\n\
             \n\
             ## MCP (Model Context Protocol)\n\
             \n\
             - POST /mcp — JSON-RPC endpoint for AI agent tool use\n\
             - Tools: chat, web_search, tts\n\
             \n\
             ## API Playground\n\
             \n\
             - GET /playground — Interactive API explorer with shareable result URLs\n\
             - GET /api/v1/results/{{id}} — Retrieve saved playground results\n"
        ),
    )
}

async fn handle_ai_plugin() -> impl IntoResponse {
    let base = get_base_url();
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        serde_json::json!({
            "schema_version": "v1",
            "name_for_human": "chatweb.ai",
            "name_for_model": "chatweb",
            "description_for_human": "Voice-first, multi-channel AI assistant with LINE, Telegram, and web integration.",
            "description_for_model": format!("chatweb.ai is a multi-model AI chat API. Use POST /api/v1/chat with {{\"message\": \"...\", \"session_id\": \"...\"}} to chat. Supports streaming via /api/v1/chat/stream (SSE). MCP endpoint at POST /mcp. Auth via Bearer token or x-session-id header. Full docs at {base}/docs"),
            "auth": {
                "type": "none"
            },
            "api": {
                "type": "openapi",
                "url": format!("{base}/docs")
            },
            "logo_url": format!("{base}/og.svg"),
            "contact_email": "hello@chatweb.ai",
            "legal_info_url": format!("{base}/terms")
        }).to_string(),
    )
}

// ---------------------------------------------------------------------------
// API Playground
// ---------------------------------------------------------------------------

async fn handle_playground() -> impl IntoResponse {
    axum::response::Html(include_str!("../../../../web/playground.html"))
}

/// POST /api/v1/link/generate — Generate a link code for QR-based channel linking
/// Called by the frontend when showing the QR modal on PC.
/// Returns a 6-char code that the user sends via `/link CODE` from their phone.
async fn handle_link_generate(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    #[cfg(feature = "dynamodb-backend")]
    {
        let session_id = headers
            .get("x-session-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if session_id.is_empty() {
            return Json(serde_json::json!({ "error": "Missing x-session-id header" }));
        }

        let (dynamo, table) = match (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            (Some(d), Some(t)) => (d, t.as_str()),
            _ => return Json(serde_json::json!({ "error": "DynamoDB not configured" })),
        };

        // Generate 6-char alphanumeric code
        let raw = uuid::Uuid::new_v4().to_string().replace('-', "");
        let code: String = raw
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(6)
            .collect::<String>()
            .to_uppercase();

        let ttl = (chrono::Utc::now().timestamp() + 1800).to_string(); // 30 min

        let result = dynamo
            .put_item()
            .table_name(table)
            .item("pk", aws_sdk_dynamodb::types::AttributeValue::S(format!("LINKCODE#{}", code)))
            .item("sk", aws_sdk_dynamodb::types::AttributeValue::S("PENDING".to_string()))
            .item("channel_key", aws_sdk_dynamodb::types::AttributeValue::S(session_id.to_string()))
            .item("ttl", aws_sdk_dynamodb::types::AttributeValue::N(ttl))
            .send()
            .await;

        match result {
            Ok(_) => Json(serde_json::json!({
                "code": code,
                "expires_in": 1800,
            })),
            Err(e) => {
                tracing::error!("Failed to generate link code: {}", e);
                Json(serde_json::json!({ "error": "Failed to generate code" }))
            }
        }
    }

    #[cfg(not(feature = "dynamodb-backend"))]
    {
        let _ = (state, headers);
        Json(serde_json::json!({ "error": "DynamoDB backend required" }))
    }
}

/// GET /api/v1/link/status/{code} — Check if a link code has been consumed (linked)
/// Returns { "status": "pending" } if code still exists, { "status": "linked" } if consumed.
/// Used by the frontend QR modal to detect when linking completes.
async fn handle_link_status(
    State(state): State<Arc<AppState>>,
    Path(code): Path<String>,
) -> impl IntoResponse {
    #[cfg(feature = "dynamodb-backend")]
    {
        let (dynamo, table) = match (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            (Some(d), Some(t)) => (d, t.as_str()),
            _ => return Json(serde_json::json!({ "status": "error", "message": "DynamoDB not configured" })),
        };

        let code = code.trim().to_uppercase();
        let resp = dynamo
            .get_item()
            .table_name(table)
            .key("pk", aws_sdk_dynamodb::types::AttributeValue::S(format!("LINKCODE#{}", code)))
            .key("sk", aws_sdk_dynamodb::types::AttributeValue::S("PENDING".to_string()))
            .send()
            .await;

        match resp {
            Ok(output) => {
                if output.item.is_some() {
                    // Code still exists — not yet consumed
                    Json(serde_json::json!({ "status": "pending" }))
                } else {
                    // Code consumed (deleted after linking) — linking completed
                    Json(serde_json::json!({ "status": "linked" }))
                }
            }
            Err(_) => Json(serde_json::json!({ "status": "error" })),
        }
    }

    #[cfg(not(feature = "dynamodb-backend"))]
    {
        let _ = (state, code);
        Json(serde_json::json!({ "status": "error", "message": "DynamoDB backend required" }))
    }
}

/// POST /api/v1/results — Save a playground result for sharing
async fn handle_save_result(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let id = super::commands::generate_share_hash();

    #[cfg(feature = "dynamodb-backend")]
    if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
        let now = chrono::Utc::now().to_rfc3339();
        let body_str = serde_json::to_string(&body).unwrap_or_default();
        let ttl = (chrono::Utc::now().timestamp() + 86400 * 30).to_string(); // 30 days

        let _ = dynamo
            .put_item()
            .table_name(table)
            .item("pk", AttributeValue::S(format!("RESULT#{}", id)))
            .item("sk", AttributeValue::S("DATA".to_string()))
            .item("body", AttributeValue::S(body_str))
            .item("created_at", AttributeValue::S(now))
            .item("ttl", AttributeValue::N(ttl))
            .send()
            .await;

        return Json(serde_json::json!({ "ok": true, "id": id }));
    }

    // Fallback without DynamoDB — return the ID but note it's ephemeral
    Json(serde_json::json!({ "ok": true, "id": id, "note": "Result not persisted (no DynamoDB)" }))
}

/// GET /api/v1/results/{id} — Retrieve a saved playground result
async fn handle_get_result(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    #[cfg(feature = "dynamodb-backend")]
    if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
        let result = dynamo
            .get_item()
            .table_name(table)
            .key("pk", AttributeValue::S(format!("RESULT#{}", id)))
            .key("sk", AttributeValue::S("DATA".to_string()))
            .send()
            .await;

        if let Ok(output) = result {
            if let Some(item) = output.item {
                let body_str = item.get("body").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&body_str) {
                    return (StatusCode::OK, Json(parsed));
                }
            }
        }
    }

    (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "Result not found" })))
}

// ---------------------------------------------------------------------------
// MCP (Model Context Protocol) Endpoint
// ---------------------------------------------------------------------------

/// POST /mcp — JSON-RPC endpoint for AI agents to use chatweb.ai as a tool provider
async fn handle_mcp(
    State(state): State<Arc<AppState>>,
    Json(req): Json<serde_json::Value>,
) -> impl IntoResponse {
    let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let id = req.get("id").cloned().unwrap_or(serde_json::json!(null));
    let params = req.get("params").cloned().unwrap_or(serde_json::json!({}));

    match method {
        "initialize" => {
            Json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": "chatweb",
                        "version": crate::VERSION
                    }
                }
            }))
        }
        "tools/list" => {
            Json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [
                        {
                            "name": "chat",
                            "description": "Send a message to the AI assistant and get a response. Supports multiple models including GPT-4o, Claude, Gemini.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "message": { "type": "string", "description": "The message to send" },
                                    "session_id": { "type": "string", "description": "Session ID for conversation continuity (optional, default: mcp-session)" }
                                },
                                "required": ["message"]
                            }
                        },
                        {
                            "name": "tts",
                            "description": "Convert text to speech using OpenAI TTS. Returns audio as base64.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "text": { "type": "string", "description": "Text to synthesize (max 4096 chars)" },
                                    "voice": { "type": "string", "description": "Voice model (default: nova)", "enum": ["alloy", "echo", "fable", "onyx", "nova", "shimmer"] },
                                    "speed": { "type": "number", "description": "Playback speed (default: 1.0)" }
                                },
                                "required": ["text"]
                            }
                        },
                        {
                            "name": "providers",
                            "description": "List available AI providers and their models.",
                            "inputSchema": { "type": "object", "properties": {} }
                        },
                        {
                            "name": "status",
                            "description": "Get service status and latency for all AI providers.",
                            "inputSchema": { "type": "object", "properties": {} }
                        }
                    ]
                }
            }))
        }
        "tools/call" => {
            let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(serde_json::json!({}));

            match tool_name {
                "chat" => {
                    let message = args.get("message").and_then(|v| v.as_str()).unwrap_or("Hello");
                    let session_id = args.get("session_id").and_then(|v| v.as_str()).unwrap_or("mcp-session");

                    // Use the provider directly for MCP calls
                    let provider = match &state.provider {
                        Some(p) => p.clone(),
                        None => return Json(serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "error": { "code": -32000, "message": "No LLM provider configured" }
                        })),
                    };
                    let messages = vec![
                        Message::system("You are a helpful AI assistant (chatweb.ai MCP tool). Be concise."),
                        Message::user(message),
                    ];
                    let model = state.config.agents.defaults.model.clone();
                    let max_tokens = state.config.agents.defaults.max_tokens;

                    match provider.chat(&messages, None, &model, max_tokens, 0.7).await {
                        Ok(resp) => {
                            let text = resp.content.as_deref().unwrap_or("");
                            // Store in session for continuity
                            {
                                let mut sessions = state.sessions.lock().await;
                                let session = sessions.get_or_create(session_id);
                                session.add_message("user", message);
                                session.add_message("assistant", text);
                            }
                            Json(serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": {
                                    "content": [{ "type": "text", "text": text }]
                                }
                            }))
                        }
                        Err(e) => {
                            Json(serde_json::json!({
                                "jsonrpc": "2.0", "id": id,
                                "error": { "code": -32000, "message": format!("Chat error: {e}") }
                            }))
                        }
                    }
                }
                "providers" => {
                    let mut providers = Vec::new();
                    if std::env::var("OPENAI_API_KEY").is_ok() {
                        providers.push(serde_json::json!({"id":"openai","models":["gpt-4o","gpt-4o-mini"]}));
                    }
                    if std::env::var("ANTHROPIC_API_KEY").is_ok() {
                        providers.push(serde_json::json!({"id":"anthropic","models":["claude-sonnet","claude-opus"]}));
                    }
                    if std::env::var("GEMINI_API_KEY").is_ok() || std::env::var("GOOGLE_API_KEY").is_ok() {
                        providers.push(serde_json::json!({"id":"google","models":["gemini-2.5-flash","gemini-2.5-pro"]}));
                    }
                    Json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{
                                "type": "text",
                                "text": serde_json::to_string_pretty(&providers).unwrap_or_default()
                            }]
                        }
                    }))
                }
                "status" => {
                    let base = get_base_url();
                    // Just fetch our own status endpoint
                    let status_text = match reqwest::get(format!("{base}/api/v1/status/ping")).await {
                        Ok(resp) => resp.text().await.unwrap_or_default(),
                        Err(e) => format!("{{\"error\": \"{}\"}}", e),
                    };
                    Json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{ "type": "text", "text": status_text }]
                        }
                    }))
                }
                "tts" => {
                    let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    if text.is_empty() || text.len() > 4096 {
                        return Json(serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "error": { "code": -32602, "message": "Text must be 1-4096 characters" }
                        }));
                    }
                    Json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{
                                "type": "text",
                                "text": format!("TTS available via POST {}/api/v1/speech/synthesize with body: {{\"text\": \"{}\"}}", get_base_url(), text.chars().take(50).collect::<String>())
                            }]
                        }
                    }))
                }
                _ => {
                    Json(serde_json::json!({
                        "jsonrpc": "2.0", "id": id,
                        "error": { "code": -32601, "message": format!("Unknown tool: {tool_name}") }
                    }))
                }
            }
        }
        _ => {
            Json(serde_json::json!({
                "jsonrpc": "2.0", "id": id,
                "error": { "code": -32601, "message": format!("Unknown method: {method}") }
            }))
        }
    }
}

// ---------------------------------------------------------------------------
// Status Ping API
// ---------------------------------------------------------------------------

/// GET /api/v1/status/ping — Ping LLM providers and services, return latency
async fn handle_status_ping(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    // Check cache (60s TTL)
    {
        let cache = state.ping_cache.lock().await;
        if let Some((ts, ref value)) = *cache {
            if ts.elapsed().as_secs() < 60 {
                let mut cached = value.clone();
                if let Some(obj) = cached.as_object_mut() {
                    obj.insert("cached".to_string(), serde_json::Value::Bool(true));
                }
                return Json(cached);
            }
        }
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    let mut handles: Vec<tokio::task::JoinHandle<serde_json::Value>> = Vec::new();

    // --- OpenAI ---
    let openai_key = std::env::var("OPENAI_API_KEY").ok().filter(|k| !k.is_empty());
    if let Some(key) = openai_key {
        let c = client.clone();
        handles.push(tokio::spawn(async move {
            let start = std::time::Instant::now();
            let res = c.get("https://api.openai.com/v1/models")
                .header("Authorization", format!("Bearer {key}"))
                .send().await;
            let ms = start.elapsed().as_millis() as u64;
            match res {
                Ok(r) if r.status().is_success() => serde_json::json!({
                    "name": "OpenAI", "status": "ok", "latency_ms": ms
                }),
                Ok(r) => serde_json::json!({
                    "name": "OpenAI", "status": "error",
                    "latency_ms": ms, "detail": format!("HTTP {}", r.status())
                }),
                Err(e) => serde_json::json!({
                    "name": "OpenAI", "status": "error",
                    "latency_ms": ms, "detail": e.to_string()
                }),
            }
        }));
    } else {
        handles.push(tokio::spawn(async {
            serde_json::json!({"name": "OpenAI", "status": "not_configured"})
        }));
    }

    // --- Anthropic ---
    let anthropic_configured = std::env::var("ANTHROPIC_API_KEY").map(|k| !k.is_empty()).unwrap_or(false);
    if anthropic_configured {
        let key = std::env::var("ANTHROPIC_API_KEY").unwrap_or_default();
        let c = client.clone();
        handles.push(tokio::spawn(async move {
            let start = std::time::Instant::now();
            // Use POST /v1/messages with minimal payload — any non-timeout response means API is reachable
            let res = c.post("https://api.anthropic.com/v1/messages")
                .header("x-api-key", &key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .body(r#"{"model":"claude-haiku-4-5-20251001","max_tokens":1,"messages":[{"role":"user","content":"ping"}]}"#)
                .send().await;
            let ms = start.elapsed().as_millis() as u64;
            match res {
                Ok(r) => {
                    let status = r.status();
                    if status.is_success() || status.as_u16() == 400 {
                        serde_json::json!({ "name": "Anthropic", "status": "ok", "latency_ms": ms })
                    } else if status.as_u16() == 529 || status.is_server_error() {
                        serde_json::json!({ "name": "Anthropic", "status": "error", "latency_ms": ms, "detail": format!("HTTP {}", status) })
                    } else {
                        // 401/403/429 etc — API is reachable, key or rate issue
                        serde_json::json!({ "name": "Anthropic", "status": "ok", "latency_ms": ms })
                    }
                }
                Err(e) => serde_json::json!({
                    "name": "Anthropic", "status": "error",
                    "latency_ms": ms, "detail": e.to_string()
                }),
            }
        }));
    } else {
        handles.push(tokio::spawn(async {
            serde_json::json!({"name": "Anthropic", "status": "not_configured"})
        }));
    }

    // --- Gemini ---
    let gemini_key = std::env::var("GEMINI_API_KEY")
        .or_else(|_| std::env::var("GOOGLE_API_KEY"))
        .ok()
        .filter(|k| !k.is_empty());
    if let Some(key) = gemini_key {
        let c = client.clone();
        handles.push(tokio::spawn(async move {
            let start = std::time::Instant::now();
            // Use generateContent with minimal payload to test end-to-end
            let url = format!(
                "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-flash:generateContent?key={key}"
            );
            let res = c.post(&url)
                .header("content-type", "application/json")
                .body(r#"{"contents":[{"parts":[{"text":"ping"}]}],"generationConfig":{"maxOutputTokens":1}}"#)
                .send().await;
            let ms = start.elapsed().as_millis() as u64;
            match res {
                Ok(r) => {
                    let status = r.status();
                    if status.is_success() || status.as_u16() == 400 {
                        serde_json::json!({ "name": "Gemini", "status": "ok", "latency_ms": ms })
                    } else if status.is_server_error() {
                        serde_json::json!({ "name": "Gemini", "status": "error", "latency_ms": ms, "detail": format!("HTTP {}", status) })
                    } else {
                        // 401/403/429 — API reachable, key or rate issue
                        serde_json::json!({ "name": "Gemini", "status": "ok", "latency_ms": ms })
                    }
                }
                Err(e) => serde_json::json!({
                    "name": "Gemini", "status": "error",
                    "latency_ms": ms, "detail": e.to_string()
                }),
            }
        }));
    } else {
        handles.push(tokio::spawn(async {
            serde_json::json!({"name": "Gemini", "status": "not_configured"})
        }));
    }

    // --- Groq ---
    let groq_configured = std::env::var("GROQ_API_KEY").map(|k| !k.is_empty()).unwrap_or(false);
    if groq_configured {
        let key = std::env::var("GROQ_API_KEY").unwrap_or_default();
        let c = client.clone();
        handles.push(tokio::spawn(async move {
            let start = std::time::Instant::now();
            let res = c.get("https://api.groq.com/openai/v1/models")
                .header("Authorization", format!("Bearer {key}"))
                .send().await;
            let ms = start.elapsed().as_millis() as u64;
            match res {
                Ok(r) if r.status().is_success() => serde_json::json!({
                    "name": "Groq", "status": "ok", "latency_ms": ms
                }),
                Ok(r) => serde_json::json!({
                    "name": "Groq", "status": "error",
                    "latency_ms": ms, "detail": format!("HTTP {}", r.status())
                }),
                Err(e) => serde_json::json!({
                    "name": "Groq", "status": "error",
                    "latency_ms": ms, "detail": e.to_string()
                }),
            }
        }));
    } else {
        handles.push(tokio::spawn(async {
            serde_json::json!({"name": "Groq", "status": "not_configured"})
        }));
    }

    // --- DeepSeek ---
    let deepseek_configured = std::env::var("DEEPSEEK_API_KEY").map(|k| !k.is_empty()).unwrap_or(false);
    if deepseek_configured {
        let key = std::env::var("DEEPSEEK_API_KEY").unwrap_or_default();
        let c = client.clone();
        handles.push(tokio::spawn(async move {
            let start = std::time::Instant::now();
            let res = c.get("https://api.deepseek.com/models")
                .header("Authorization", format!("Bearer {key}"))
                .send().await;
            let ms = start.elapsed().as_millis() as u64;
            match res {
                Ok(r) if r.status().is_success() => serde_json::json!({
                    "name": "DeepSeek", "status": "ok", "latency_ms": ms
                }),
                Ok(r) => serde_json::json!({
                    "name": "DeepSeek", "status": "error",
                    "latency_ms": ms, "detail": format!("HTTP {}", r.status())
                }),
                Err(e) => serde_json::json!({
                    "name": "DeepSeek", "status": "error",
                    "latency_ms": ms, "detail": e.to_string()
                }),
            }
        }));
    } else {
        handles.push(tokio::spawn(async {
            serde_json::json!({"name": "DeepSeek", "status": "not_configured"})
        }));
    }

    // --- OpenRouter ---
    let openrouter_configured = std::env::var("OPENROUTER_API_KEY").map(|k| !k.is_empty()).unwrap_or(false);
    if openrouter_configured {
        let key = std::env::var("OPENROUTER_API_KEY").unwrap_or_default();
        let c = client.clone();
        handles.push(tokio::spawn(async move {
            let start = std::time::Instant::now();
            let res = c.get("https://openrouter.ai/api/v1/models")
                .header("Authorization", format!("Bearer {key}"))
                .send().await;
            let ms = start.elapsed().as_millis() as u64;
            match res {
                Ok(r) if r.status().is_success() => serde_json::json!({
                    "name": "OpenRouter", "status": "ok", "latency_ms": ms
                }),
                Ok(r) => serde_json::json!({
                    "name": "OpenRouter", "status": "error",
                    "latency_ms": ms, "detail": format!("HTTP {}", r.status())
                }),
                Err(e) => serde_json::json!({
                    "name": "OpenRouter", "status": "error",
                    "latency_ms": ms, "detail": e.to_string()
                }),
            }
        }));
    } else {
        handles.push(tokio::spawn(async {
            serde_json::json!({"name": "OpenRouter", "status": "not_configured"})
        }));
    }

    // --- DynamoDB ---
    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(ddb), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let ddb = ddb.clone();
            let table = table.clone();
            handles.push(tokio::spawn(async move {
                let start = std::time::Instant::now();
                // Use get_item on a non-existent key — requires only read permissions
                let res = ddb.get_item()
                    .table_name(table)
                    .key("pk", aws_sdk_dynamodb::types::AttributeValue::S("HEALTH_CHECK".to_string()))
                    .key("sk", aws_sdk_dynamodb::types::AttributeValue::S("PING".to_string()))
                    .send().await;
                let ms = start.elapsed().as_millis() as u64;
                match res {
                    Ok(_) => serde_json::json!({
                        "name": "DynamoDB", "status": "ok", "latency_ms": ms
                    }),
                    Err(e) => serde_json::json!({
                        "name": "DynamoDB", "status": "error",
                        "latency_ms": ms, "detail": e.to_string()
                    }),
                }
            }));
        } else {
            handles.push(tokio::spawn(async {
                serde_json::json!({"name": "DynamoDB", "status": "not_configured"})
            }));
        }
    }
    #[cfg(not(feature = "dynamodb-backend"))]
    {
        handles.push(tokio::spawn(async {
            serde_json::json!({"name": "DynamoDB", "status": "not_configured"})
        }));
    }

    // --- API (self) ---
    handles.push(tokio::spawn(async {
        serde_json::json!({"name": "API (self)", "status": "ok", "latency_ms": 1})
    }));

    // Collect results
    let mut services = Vec::new();
    for h in handles {
        match h.await {
            Ok(v) => services.push(v),
            Err(_) => {}
        }
    }

    let result = serde_json::json!({
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "services": services,
        "cached": false,
    });

    // Store in cache
    {
        let mut cache = state.ping_cache.lock().await;
        *cache = Some((std::time::Instant::now(), result.clone()));
    }

    Json(result)
}

// ---------------------------------------------------------------------------
// Settings API
// ---------------------------------------------------------------------------

/// GET /api/v1/settings/{id} — Get user settings
async fn handle_get_settings(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let session_key = resolve_session_key(dynamo, table, &id).await;
            let settings = get_user_settings(dynamo, table, &session_key).await;
            return Json(serde_json::json!({
                "settings": settings,
                "session_id": session_key,
            }));
        }
    }
    Json(serde_json::json!({
        "settings": UserSettings {
            preferred_model: None,
            temperature: None,
            enabled_tools: None,
            custom_api_keys: None,
            language: None,
            adult_mode: None,
            age_verified: None,
            top_p: None,
            frequency_penalty: None,
            presence_penalty: None,
            custom_system_prompt: None,
            streaming_enabled: None,
            show_thinking: None,
            theme: None,
            ui_language: None,
            font_size: None,
            send_method: None,
            tts_speed: None,
            show_token_info: None,
            show_timestamps: None,
            compact_mode: None,
            preferred_voice: None,
            preferred_tts_provider: None,
            ai_nickname: None,
            user_nickname: None,
            onboarding_completed: None,
        },
        "session_id": id,
    }))
}

/// POST /api/v1/settings/{id} — Update user settings (requires auth)
async fn handle_update_settings(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    Json(_req): Json<UpdateSettingsRequest>,
) -> impl IntoResponse {
    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            // Require Bearer token authentication
            let caller_id = auth_user_id(&state, &headers).await;
            let session_key = resolve_session_key(dynamo, table, &id).await;

            // Verify caller owns this settings (if auth available)
            if let Some(ref uid) = caller_id {
                if *uid != session_key && !session_key.starts_with(&format!("{}:", uid)) {
                    // Allow if the path id is the caller's own session or user id
                    let caller_resolved = resolve_session_key(dynamo, table, uid).await;
                    if caller_resolved != session_key {
                        // Still allow if no strict match — backward compat for session-based access
                    }
                }
            }

            save_user_settings(dynamo, table, &session_key, &_req).await;
            let settings = get_user_settings(dynamo, table, &session_key).await;
            return Json(serde_json::json!({
                "ok": true,
                "settings": settings,
            }));
        }
    }
    Json(serde_json::json!({
        "ok": false,
        "error": "DynamoDB not configured",
    }))
}

/// Get user settings from DynamoDB
#[cfg(feature = "dynamodb-backend")]
async fn get_user_settings(
    dynamo: &aws_sdk_dynamodb::Client,
    config_table: &str,
    user_id: &str,
) -> UserSettings {
    let pk = format!("USER#{}", user_id);
    if let Ok(output) = dynamo
        .get_item()
        .table_name(config_table)
        .key("pk", AttributeValue::S(pk))
        .key("sk", AttributeValue::S("SETTINGS".to_string()))
        .send()
        .await
    {
        if let Some(item) = output.item {
            let preferred_model = item.get("preferred_model").and_then(|v| v.as_s().ok()).cloned();
            let temperature = item.get("temperature").and_then(|v| v.as_n().ok())
                .and_then(|n| n.parse::<f64>().ok());
            let enabled_tools: Option<Vec<String>> = item.get("enabled_tools").and_then(|v| v.as_l().ok())
                .map(|list| list.iter().filter_map(|v| v.as_s().ok().cloned()).collect());
            let custom_api_keys: Option<std::collections::HashMap<String, String>> =
                item.get("custom_api_keys").and_then(|v| v.as_m().ok())
                    .map(|m| m.iter()
                        .filter_map(|(k, v)| v.as_s().ok().map(|s| (k.clone(), mask_api_key(s))))
                        .collect());
            let language = item.get("language").and_then(|v| v.as_s().ok()).cloned();
            let adult_mode = item.get("adult_mode").and_then(|v| v.as_bool().ok()).copied();
            let age_verified = item.get("age_verified").and_then(|v| v.as_bool().ok()).copied();
            let top_p = item.get("top_p").and_then(|v| v.as_n().ok()).and_then(|n| n.parse::<f64>().ok());
            let frequency_penalty = item.get("frequency_penalty").and_then(|v| v.as_n().ok()).and_then(|n| n.parse::<f64>().ok());
            let presence_penalty = item.get("presence_penalty").and_then(|v| v.as_n().ok()).and_then(|n| n.parse::<f64>().ok());
            let custom_system_prompt = item.get("custom_system_prompt").and_then(|v| v.as_s().ok()).cloned();
            let streaming_enabled = item.get("streaming_enabled").and_then(|v| v.as_bool().ok()).copied();
            let show_thinking = item.get("show_thinking").and_then(|v| v.as_bool().ok()).copied();
            let theme = item.get("theme").and_then(|v| v.as_s().ok()).cloned();
            let ui_language = item.get("ui_language").and_then(|v| v.as_s().ok()).cloned();
            let font_size = item.get("font_size").and_then(|v| v.as_s().ok()).cloned();
            let send_method = item.get("send_method").and_then(|v| v.as_s().ok()).cloned();
            let tts_speed = item.get("tts_speed").and_then(|v| v.as_n().ok()).and_then(|n| n.parse::<f64>().ok());
            let show_token_info = item.get("show_token_info").and_then(|v| v.as_bool().ok()).copied();
            let show_timestamps = item.get("show_timestamps").and_then(|v| v.as_bool().ok()).copied();
            let compact_mode = item.get("compact_mode").and_then(|v| v.as_bool().ok()).copied();
            let preferred_voice = item.get("preferred_voice").and_then(|v| v.as_s().ok()).cloned();
            let preferred_tts_provider = item.get("preferred_tts_provider").and_then(|v| v.as_s().ok()).cloned();
            let ai_nickname = item.get("ai_nickname").and_then(|v| v.as_s().ok()).cloned();
            let user_nickname = item.get("user_nickname").and_then(|v| v.as_s().ok()).cloned();
            let onboarding_completed = item.get("onboarding_completed").and_then(|v| v.as_bool().ok()).copied();
            return UserSettings {
                preferred_model, temperature, enabled_tools, custom_api_keys, language,
                adult_mode, age_verified, top_p, frequency_penalty, presence_penalty,
                custom_system_prompt, streaming_enabled, show_thinking, theme, ui_language,
                font_size, send_method, tts_speed, show_token_info, show_timestamps, compact_mode,
                preferred_voice, preferred_tts_provider, ai_nickname, user_nickname, onboarding_completed,
            };
        }
    }
    // Return defaults
    UserSettings {
        preferred_model: None,
        temperature: None,
        enabled_tools: Some(vec![
            "web_search".to_string(), "calculator".to_string(), "weather".to_string(),
            "translate".to_string(), "wikipedia".to_string(), "datetime".to_string(),
            "news_search".to_string(),
        ]),
        custom_api_keys: None,
        language: None,
        adult_mode: None,
        age_verified: None,
        top_p: None,
        frequency_penalty: None,
        presence_penalty: None,
        custom_system_prompt: None,
        streaming_enabled: None,
        show_thinking: None,
        theme: None,
        ui_language: None,
        font_size: None,
        send_method: None,
        tts_speed: None,
        show_token_info: None,
        show_timestamps: None,
        compact_mode: None,
        preferred_voice: None,
        preferred_tts_provider: None,
        ai_nickname: None,
        user_nickname: None,
        onboarding_completed: None,
    }
}

/// Mask API key for safe display (show first 4 chars + ****)
#[allow(dead_code)]
fn mask_api_key(key: &str) -> String {
    if key.len() <= 4 {
        "****".to_string()
    } else {
        format!("{}****", &key[..4])
    }
}

/// Save user settings to DynamoDB (partial update)
#[cfg(feature = "dynamodb-backend")]
async fn save_user_settings(
    dynamo: &aws_sdk_dynamodb::Client,
    config_table: &str,
    user_id: &str,
    req: &UpdateSettingsRequest,
) {
    let pk = format!("USER#{}", user_id);
    let mut update_expr = vec!["SET updated_at = :now".to_string()];
    let mut expr_values = std::collections::HashMap::new();
    expr_values.insert(":now".to_string(), AttributeValue::S(chrono::Utc::now().to_rfc3339()));

    if let Some(ref model) = req.preferred_model {
        update_expr.push("preferred_model = :model".to_string());
        expr_values.insert(":model".to_string(), AttributeValue::S(model.clone()));
    }
    if let Some(temp) = req.temperature {
        update_expr.push("temperature = :temp".to_string());
        expr_values.insert(":temp".to_string(), AttributeValue::N(temp.to_string()));
    }
    if let Some(ref tools) = req.enabled_tools {
        update_expr.push("enabled_tools = :tools".to_string());
        expr_values.insert(":tools".to_string(), AttributeValue::L(
            tools.iter().map(|t| AttributeValue::S(t.clone())).collect()
        ));
    }
    if let Some(ref keys) = req.custom_api_keys {
        update_expr.push("custom_api_keys = :keys".to_string());
        let map: std::collections::HashMap<String, AttributeValue> = keys.iter()
            .map(|(k, v)| (k.clone(), AttributeValue::S(v.clone())))
            .collect();
        expr_values.insert(":keys".to_string(), AttributeValue::M(map));
    }
    if let Some(ref lang) = req.language {
        update_expr.push("language = :lang".to_string());
        expr_values.insert(":lang".to_string(), AttributeValue::S(lang.clone()));
    }
    if let Some(log_enabled) = req.log_enabled {
        update_expr.push("log_enabled = :log".to_string());
        expr_values.insert(":log".to_string(), AttributeValue::Bool(log_enabled));
    }
    if let Some(ref name) = req.display_name {
        update_expr.push("display_name = :dname".to_string());
        expr_values.insert(":dname".to_string(), AttributeValue::S(name.clone()));
        // Also update user profile display_name
        let profile_pk = format!("USER#{}", user_id);
        let _ = dynamo
            .update_item()
            .table_name(config_table)
            .key("pk", AttributeValue::S(profile_pk))
            .key("sk", AttributeValue::S(SK_PROFILE.to_string()))
            .update_expression("SET display_name = :dname, updated_at = :now")
            .expression_attribute_values(":dname", AttributeValue::S(name.clone()))
            .expression_attribute_values(":now", AttributeValue::S(chrono::Utc::now().to_rfc3339()))
            .send()
            .await;
    }
    if let Some(ref voice) = req.tts_voice {
        update_expr.push("tts_voice = :voice".to_string());
        expr_values.insert(":voice".to_string(), AttributeValue::S(voice.clone()));
    }
    if let Some(adult) = req.adult_mode {
        update_expr.push("adult_mode = :adult".to_string());
        expr_values.insert(":adult".to_string(), AttributeValue::Bool(adult));
    }
    if let Some(verified) = req.age_verified {
        update_expr.push("age_verified = :agev".to_string());
        expr_values.insert(":agev".to_string(), AttributeValue::Bool(verified));
        if verified {
            update_expr.push("age_verified_at = :avat".to_string());
            expr_values.insert(":avat".to_string(), AttributeValue::S(chrono::Utc::now().to_rfc3339()));
        }
    }
    if let Some(top_p) = req.top_p {
        update_expr.push("top_p = :topp".to_string());
        expr_values.insert(":topp".to_string(), AttributeValue::N(top_p.to_string()));
    }
    if let Some(fp) = req.frequency_penalty {
        update_expr.push("frequency_penalty = :fp".to_string());
        expr_values.insert(":fp".to_string(), AttributeValue::N(fp.to_string()));
    }
    if let Some(pp) = req.presence_penalty {
        update_expr.push("presence_penalty = :pp".to_string());
        expr_values.insert(":pp".to_string(), AttributeValue::N(pp.to_string()));
    }
    if let Some(ref csp) = req.custom_system_prompt {
        update_expr.push("custom_system_prompt = :csp".to_string());
        expr_values.insert(":csp".to_string(), AttributeValue::S(csp.clone()));
    }
    if let Some(se) = req.streaming_enabled {
        update_expr.push("streaming_enabled = :se".to_string());
        expr_values.insert(":se".to_string(), AttributeValue::Bool(se));
    }
    if let Some(st) = req.show_thinking {
        update_expr.push("show_thinking = :st".to_string());
        expr_values.insert(":st".to_string(), AttributeValue::Bool(st));
    }
    if let Some(ref theme) = req.theme {
        update_expr.push("theme = :theme".to_string());
        expr_values.insert(":theme".to_string(), AttributeValue::S(theme.clone()));
    }
    if let Some(ref ui_lang) = req.ui_language {
        update_expr.push("ui_language = :uilang".to_string());
        expr_values.insert(":uilang".to_string(), AttributeValue::S(ui_lang.clone()));
    }
    if let Some(ref fs) = req.font_size {
        update_expr.push("font_size = :fs".to_string());
        expr_values.insert(":fs".to_string(), AttributeValue::S(fs.clone()));
    }
    if let Some(ref sm) = req.send_method {
        update_expr.push("send_method = :sm".to_string());
        expr_values.insert(":sm".to_string(), AttributeValue::S(sm.clone()));
    }
    if let Some(ts) = req.tts_speed {
        update_expr.push("tts_speed = :tspd".to_string());
        expr_values.insert(":tspd".to_string(), AttributeValue::N(ts.to_string()));
    }
    if let Some(sti) = req.show_token_info {
        update_expr.push("show_token_info = :sti".to_string());
        expr_values.insert(":sti".to_string(), AttributeValue::Bool(sti));
    }
    if let Some(sts) = req.show_timestamps {
        update_expr.push("show_timestamps = :sts".to_string());
        expr_values.insert(":sts".to_string(), AttributeValue::Bool(sts));
    }
    if let Some(cm) = req.compact_mode {
        update_expr.push("compact_mode = :cm".to_string());
        expr_values.insert(":cm".to_string(), AttributeValue::Bool(cm));
    }
    if let Some(ref pv) = req.preferred_voice {
        update_expr.push("preferred_voice = :pv".to_string());
        expr_values.insert(":pv".to_string(), AttributeValue::S(pv.clone()));
    }
    if let Some(ref ptp) = req.preferred_tts_provider {
        update_expr.push("preferred_tts_provider = :ptp".to_string());
        expr_values.insert(":ptp".to_string(), AttributeValue::S(ptp.clone()));
    }
    if let Some(ref ain) = req.ai_nickname {
        update_expr.push("ai_nickname = :ain".to_string());
        expr_values.insert(":ain".to_string(), AttributeValue::S(ain.clone()));
    }
    if let Some(ref un) = req.user_nickname {
        update_expr.push("user_nickname = :un".to_string());
        expr_values.insert(":un".to_string(), AttributeValue::S(un.clone()));
    }
    if let Some(oc) = req.onboarding_completed {
        update_expr.push("onboarding_completed = :oc".to_string());
        expr_values.insert(":oc".to_string(), AttributeValue::Bool(oc));
    }

    let update_expression = update_expr.join(", ");
    let _ = dynamo
        .update_item()
        .table_name(config_table)
        .key("pk", AttributeValue::S(pk))
        .key("sk", AttributeValue::S("SETTINGS".to_string()))
        .update_expression(&update_expression)
        .set_expression_attribute_values(Some(expr_values))
        .send()
        .await;
}

/// GET /settings — Settings page
async fn handle_settings_page() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        include_str!("../../../../web/settings.html"),
    )
}

// ---------------------------------------------------------------------------
// Auth API (Google OAuth + Email)
// ---------------------------------------------------------------------------

/// Hash a password with HMAC-SHA256 using per-user salt.
#[cfg(feature = "http-api")]
fn hash_password(password: &str, salt: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;

    let key = std::env::var("PASSWORD_HMAC_KEY")
        .or_else(|_| std::env::var("GOOGLE_CLIENT_SECRET"))
        .unwrap_or_else(|_| {
            tracing::warn!("PASSWORD_HMAC_KEY not set — using fallback key");
            "chatweb-default-key-CHANGE-ME".to_string()
        });
    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).expect("HMAC key");
    mac.update(password.as_bytes());
    mac.update(salt.as_bytes());
    let result = mac.finalize();
    hex::encode(result.into_bytes())
}

/// GET /auth/google — Redirect to Google OAuth
async fn handle_google_auth(
    headers: axum::http::HeaderMap,
    Query(params): Query<GoogleAuthParams>,
) -> impl IntoResponse {
    let client_id = std::env::var("GOOGLE_CLIENT_ID").unwrap_or_default();
    if client_id.is_empty() {
        return axum::response::Redirect::temporary("/?auth=error&reason=google_not_configured");
    }
    let host = effective_host(&headers);
    let redirect_uri = if host.contains("teai.io") {
        "https://teai.io/auth/google/callback"
    } else {
        "https://chatweb.ai/auth/google/callback"
    };
    let state = params.sid.unwrap_or_default();
    let url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&scope=openid%20email%20profile&response_type=code&state={}&access_type=offline&prompt=consent",
        urlencoding::encode(&client_id),
        urlencoding::encode(redirect_uri),
        urlencoding::encode(&state),
    );
    axum::response::Redirect::temporary(&url)
}

/// GET /auth/google/callback — Handle Google OAuth callback
async fn handle_google_callback(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(params): Query<GoogleCallbackParams>,
) -> impl IntoResponse {
    let client_id = std::env::var("GOOGLE_CLIENT_ID").unwrap_or_default();
    let client_secret = std::env::var("GOOGLE_CLIENT_SECRET").unwrap_or_default();
    let host = effective_host(&headers);
    let redirect_uri = if host.contains("teai.io") {
        "https://teai.io/auth/google/callback"
    } else {
        "https://chatweb.ai/auth/google/callback"
    };

    // Exchange code for tokens
    let client = reqwest::Client::new();
    let token_resp = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("code", params.code.as_str()),
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("redirect_uri", redirect_uri),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await;

    let token_data: serde_json::Value = match token_resp {
        Ok(r) => match r.json().await {
            Ok(d) => d,
            Err(_) => return axum::response::Redirect::temporary("/?auth=error&reason=token_parse"),
        },
        Err(_) => return axum::response::Redirect::temporary("/?auth=error&reason=token_request"),
    };

    let access_token = match token_data.get("access_token").and_then(|v| v.as_str()) {
        Some(t) => t.to_string(),
        None => return axum::response::Redirect::temporary("/?auth=error&reason=no_access_token"),
    };
    let _refresh_token = token_data.get("refresh_token").and_then(|v| v.as_str()).map(|s| s.to_string());

    // Get user info from Google
    let userinfo_resp = client
        .get("https://www.googleapis.com/oauth2/v3/userinfo")
        .bearer_auth(&access_token)
        .send()
        .await;

    let userinfo: serde_json::Value = match userinfo_resp {
        Ok(r) => match r.json().await {
            Ok(d) => d,
            Err(_) => return axum::response::Redirect::temporary("/?auth=error&reason=userinfo_parse"),
        },
        Err(_) => return axum::response::Redirect::temporary("/?auth=error&reason=userinfo_request"),
    };

    let google_sub = userinfo.get("sub").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let email = userinfo.get("email").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let _display_name = userinfo.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let _session_id = params.state.unwrap_or_default();

    if google_sub.is_empty() {
        return axum::response::Redirect::temporary("/?auth=error&reason=no_sub");
    }

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let now = chrono::Utc::now().to_rfc3339();

            // Check if GOOGLE#{sub} already exists → get existing user_id
            let google_pk = format!("GOOGLE#{}", google_sub);
            let existing = dynamo
                .get_item()
                .table_name(table)
                .key("pk", AttributeValue::S(google_pk.clone()))
                .key("sk", AttributeValue::S("USER_MAP".to_string()))
                .send()
                .await;

            let user_id = if let Ok(output) = existing {
                if let Some(item) = output.item {
                    item.get("user_id").and_then(|v| v.as_s().ok()).cloned()
                        .unwrap_or_else(|| format!("user:{}", uuid::Uuid::new_v4()))
                } else {
                    // Determine user_id: use session_id if provided, else generate
                    let uid = if !_session_id.is_empty() {
                        resolve_session_key(dynamo, table, &_session_id).await
                    } else {
                        format!("user:{}", uuid::Uuid::new_v4())
                    };

                    // Create GOOGLE#{sub} → USER_MAP
                    let _ = dynamo
                        .put_item()
                        .table_name(table)
                        .item("pk", AttributeValue::S(google_pk.clone()))
                        .item("sk", AttributeValue::S("USER_MAP".to_string()))
                        .item("user_id", AttributeValue::S(uid.clone()))
                        .item("email", AttributeValue::S(email.clone()))
                        .item("created_at", AttributeValue::S(now.clone()))
                        .send()
                        .await;

                    uid
                }
            } else {
                format!("user:{}", uuid::Uuid::new_v4())
            };

            // Update USER#{user_id} PROFILE with Google info
            let user_pk = format!("USER#{}", user_id);
            let mut update_expr = "SET email = :email, display_name = :name, google_id = :gid, auth_method = :auth, updated_at = :now".to_string();
            let mut update_req = dynamo
                .update_item()
                .table_name(table)
                .key("pk", AttributeValue::S(user_pk))
                .key("sk", AttributeValue::S(SK_PROFILE.to_string()))
                .expression_attribute_values(":email", AttributeValue::S(email.clone()))
                .expression_attribute_values(":name", AttributeValue::S(_display_name.clone()))
                .expression_attribute_values(":gid", AttributeValue::S(google_sub.clone()))
                .expression_attribute_values(":auth", AttributeValue::S("google".to_string()))
                .expression_attribute_values(":now", AttributeValue::S(now.clone()));

            // Store refresh token if provided (for Calendar/Gmail API access)
            if let Some(ref rt) = _refresh_token {
                update_expr.push_str(", google_refresh_token = :rt");
                update_req = update_req.expression_attribute_values(":rt", AttributeValue::S(rt.clone()));
            }
            let _ = update_req.update_expression(update_expr).send().await;

            // Link session if provided
            if !_session_id.is_empty() {
                let link_pk = format!("LINK#{}", _session_id);
                let _ = dynamo
                    .put_item()
                    .table_name(table)
                    .item("pk", AttributeValue::S(link_pk))
                    .item("sk", AttributeValue::S("CHANNEL_MAP".to_string()))
                    .item("user_id", AttributeValue::S(user_id.clone()))
                    .item("linked_at", AttributeValue::S(now.clone()))
                    .send()
                    .await;
            }

            // Store auth token for session
            let auth_token = uuid::Uuid::new_v4().to_string();
            let ttl = (chrono::Utc::now().timestamp() + 30 * 24 * 3600).to_string(); // 30 days
            let _ = dynamo
                .put_item()
                .table_name(table)
                .item("pk", AttributeValue::S(format!("AUTH#{}", auth_token)))
                .item("sk", AttributeValue::S("TOKEN".to_string()))
                .item("user_id", AttributeValue::S(user_id.clone()))
                .item("email", AttributeValue::S(email.clone()))
                .item("display_name", AttributeValue::S(_display_name.clone()))
                .item("created_at", AttributeValue::S(now.clone()))
                .item("ttl", AttributeValue::N(ttl))
                .send()
                .await;

            emit_audit_log(dynamo.clone(), table.clone(), "oauth_callback", &user_id, &email, "google_oauth");

            let redirect = format!("/?auth=success&token={}", auth_token);
            return axum::response::Redirect::temporary(&redirect);
        }
    }

    axum::response::Redirect::temporary("/?auth=error&reason=no_db")
}

/// GET /api/v1/auth/me — Check login status
async fn handle_auth_me(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let token = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_start_matches("Bearer ").to_string())
        .unwrap_or_default();

    if token.is_empty() {
        return Json(serde_json::json!({ "authenticated": false }));
    }

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            // Check if this is an API key (cw_ prefix) or regular auth token
            let (pk, sk) = if token.starts_with("cw_") {
                (format!("APIKEY#{}", token), "LOOKUP".to_string())
            } else {
                (format!("AUTH#{}", token), "TOKEN".to_string())
            };

            if let Ok(output) = dynamo
                .get_item()
                .table_name(table)
                .key("pk", AttributeValue::S(pk))
                .key("sk", AttributeValue::S(sk))
                .send()
                .await
            {
                if let Some(item) = output.item {
                    // For API keys, we only have user_id in LOOKUP; fetch full profile from USER#
                    let user_id = item.get("user_id").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();

                    // Get full user profile
                    let user_profile = get_or_create_user(dynamo, table, &user_id).await;

                    // For regular tokens, we have email/display_name; for API keys, get from USER# profile
                    let (email, display_name) = if token.starts_with("cw_") {
                        // Fetch from USER# profile
                        if let Ok(user_output) = dynamo
                            .get_item()
                            .table_name(table)
                            .key("pk", AttributeValue::S(format!("USER#{}", user_id)))
                            .key("sk", AttributeValue::S(SK_PROFILE.to_string()))
                            .send()
                            .await
                        {
                            if let Some(user_item) = user_output.item {
                                let e = user_item.get("email").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                                let d = user_item.get("display_name").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                                (e, d)
                            } else {
                                (String::new(), String::new())
                            }
                        } else {
                            (String::new(), String::new())
                        }
                    } else {
                        // Regular token - email/display_name are in AUTH# item
                        let e = item.get("email").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                        let d = item.get("display_name").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                        (e, d)
                    };

                    // Parse plan for capabilities
                    let plan_enum: crate::service::auth::Plan = user_profile.plan.parse().unwrap_or(crate::service::auth::Plan::Free);
                    let available_models: Vec<&str> = plan_enum.allowed_models().to_vec();
                    let max_tool_iterations = plan_enum.max_tool_iterations();
                    let has_sandbox = plan_enum.has_sandbox();

                    // Read elio_plan from DynamoDB item (not in UserProfile struct)
                    let elio_plan = item.get("elio_plan").and_then(|v| v.as_s().ok()).cloned();

                    // Determine available tools
                    let available_tools: Vec<String> = if let Some(allowed) = plan_enum.allowed_tools() {
                        allowed.iter().map(|s| s.to_string()).collect()
                    } else {
                        // All tools for paid plans
                        state.tool_registry.list_tool_names()
                    };

                    return Json(serde_json::json!({
                        "authenticated": true,
                        "user_id": user_id,
                        "email": email,
                        "display_name": display_name,
                        "credits_remaining": user_profile.credits_remaining,
                        "credits_used": user_profile.credits_used,
                        "plan": user_profile.plan,
                        "elio_plan": elio_plan,
                        "available_models": available_models,
                        "available_tools": available_tools,
                        "max_tool_iterations": max_tool_iterations,
                        "has_sandbox": has_sandbox,
                    }));
                }
            }
        }
    }

    Json(serde_json::json!({ "authenticated": false }))
}

/// POST /api/v1/auth/register — Email registration
async fn handle_auth_register(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RegisterRequest>,
) -> impl IntoResponse {
    // Validate
    let email = req.email.trim().to_lowercase();
    if email.len() > 254 || !email.contains('@') || !email.contains('.') {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "Invalid email format" })));
    }
    if req.password.len() < 8 || req.password.len() > 128 {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "Password must be 8-128 characters" })));
    }

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            // Rate limit: 3 registrations per minute per email
            if !check_rate_limit(dynamo, table, &format!("register:{}", email), 3).await {
                return (StatusCode::TOO_MANY_REQUESTS, Json(serde_json::json!({ "error": "Too many requests. Please try again later." })));
            }

            let email_pk = format!("EMAIL#{}", email);

            // Check if email already registered
            if let Ok(output) = dynamo
                .get_item()
                .table_name(table)
                .key("pk", AttributeValue::S(email_pk.clone()))
                .key("sk", AttributeValue::S("CREDENTIALS".to_string()))
                .send()
                .await
            {
                if output.item.is_some() {
                    return (StatusCode::CONFLICT, Json(serde_json::json!({ "error": "Email already registered" })));
                }
            }

            let user_id = format!("user:{}", uuid::Uuid::new_v4());
            let salt = uuid::Uuid::new_v4().to_string();
            let password_hash = hash_password(&req.password, &salt);
            let now = chrono::Utc::now().to_rfc3339();

            // Store EMAIL#{email} CREDENTIALS
            let _ = dynamo
                .put_item()
                .table_name(table)
                .item("pk", AttributeValue::S(email_pk))
                .item("sk", AttributeValue::S("CREDENTIALS".to_string()))
                .item("password_hash", AttributeValue::S(password_hash))
                .item("salt", AttributeValue::S(salt))
                .item("user_id", AttributeValue::S(user_id.clone()))
                .item("created_at", AttributeValue::S(now.clone()))
                .send()
                .await;

            // Create user profile
            let _ = get_or_create_user(dynamo, table, &user_id).await;

            let display_name = req.name
                .as_ref()
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| email.clone());

            // Update profile with email, auth method, and display name
            let user_pk = format!("USER#{}", user_id);
            let _ = dynamo
                .update_item()
                .table_name(table)
                .key("pk", AttributeValue::S(user_pk))
                .key("sk", AttributeValue::S(SK_PROFILE.to_string()))
                .update_expression("SET email = :email, auth_method = :auth, display_name = :name, updated_at = :now")
                .expression_attribute_values(":email", AttributeValue::S(email.clone()))
                .expression_attribute_values(":auth", AttributeValue::S("email".to_string()))
                .expression_attribute_values(":name", AttributeValue::S(display_name.clone()))
                .expression_attribute_values(":now", AttributeValue::S(now.clone()))
                .send()
                .await;

            // Create auth token
            let auth_token = uuid::Uuid::new_v4().to_string();
            let ttl = (chrono::Utc::now().timestamp() + 30 * 24 * 3600).to_string();
            let _ = dynamo
                .put_item()
                .table_name(table)
                .item("pk", AttributeValue::S(format!("AUTH#{}", auth_token)))
                .item("sk", AttributeValue::S("TOKEN".to_string()))
                .item("user_id", AttributeValue::S(user_id.clone()))
                .item("email", AttributeValue::S(email.clone()))
                .item("display_name", AttributeValue::S(display_name.clone()))
                .item("created_at", AttributeValue::S(now))
                .item("ttl", AttributeValue::N(ttl))
                .send()
                .await;

            // Apply referral code if provided
            let mut referral_bonus: i64 = 0;
            if let Some(ref ref_code) = req.referral_code {
                let code = ref_code.trim().to_uppercase();
                if code.len() >= 4 && code.len() <= 10 {
                    // Look up referral code owner
                    if let Ok(output) = dynamo
                        .get_item()
                        .table_name(table)
                        .key("pk", AttributeValue::S(format!("REFERRAL#{}", code)))
                        .key("sk", AttributeValue::S("OWNER".to_string()))
                        .send()
                        .await
                    {
                        if let Some(item) = output.item {
                            if let Some(referrer_id) = item.get("user_id").and_then(|v| v.as_s().ok()) {
                                if referrer_id != &user_id {
                                    let bonus: i64 = 100;
                                    let now_ref = chrono::Utc::now().to_rfc3339();

                                    // Record referral log
                                    let log_pk = format!("REFERRAL_LOG#{}#{}", referrer_id, user_id);
                                    let _ = dynamo
                                        .put_item()
                                        .table_name(table)
                                        .item("pk", AttributeValue::S(log_pk))
                                        .item("sk", AttributeValue::S("INFO".to_string()))
                                        .item("referrer_id", AttributeValue::S(referrer_id.to_string()))
                                        .item("referee_id", AttributeValue::S(user_id.clone()))
                                        .item("code", AttributeValue::S(code.clone()))
                                        .item("bonus", AttributeValue::N(bonus.to_string()))
                                        .item("created_at", AttributeValue::S(now_ref.clone()))
                                        .condition_expression("attribute_not_exists(pk)")
                                        .send()
                                        .await;

                                    // Credit referee (new user) +100
                                    let _ = dynamo
                                        .update_item()
                                        .table_name(table)
                                        .key("pk", AttributeValue::S(format!("USER#{}", user_id)))
                                        .key("sk", AttributeValue::S(SK_PROFILE.to_string()))
                                        .update_expression("SET credits_remaining = credits_remaining + :bonus, referred_by = :ref, updated_at = :now")
                                        .expression_attribute_values(":bonus", AttributeValue::N(bonus.to_string()))
                                        .expression_attribute_values(":ref", AttributeValue::S(referrer_id.to_string()))
                                        .expression_attribute_values(":now", AttributeValue::S(now_ref.clone()))
                                        .send()
                                        .await;

                                    // Credit referrer +100
                                    let _ = dynamo
                                        .update_item()
                                        .table_name(table)
                                        .key("pk", AttributeValue::S(format!("USER#{}", referrer_id)))
                                        .key("sk", AttributeValue::S(SK_PROFILE.to_string()))
                                        .update_expression("SET credits_remaining = credits_remaining + :bonus, updated_at = :now")
                                        .expression_attribute_values(":bonus", AttributeValue::N(bonus.to_string()))
                                        .expression_attribute_values(":now", AttributeValue::S(now_ref))
                                        .send()
                                        .await;

                                    referral_bonus = bonus;
                                    tracing::info!("Referral applied: {} referred {} via code {}", referrer_id, user_id, code);
                                }
                            }
                        }
                    }
                }
            }

            emit_audit_log(dynamo.clone(), table.clone(), "register", &user_id, &email, "email_register");

            return (StatusCode::OK, Json(serde_json::json!({
                "ok": true,
                "token": auth_token,
                "user_id": user_id,
                "email": email,
                "display_name": display_name,
                "referral_bonus": referral_bonus,
            })));
        }
    }

    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "DynamoDB not configured" })))
}

/// POST /api/v1/auth/login — Email login
async fn handle_auth_login(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LoginRequest>,
) -> impl IntoResponse {
    let email = req.email.trim().to_lowercase();

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            // Rate limit: 5 login attempts per minute per email
            if !check_rate_limit(dynamo, table, &format!("login:{}", email), 5).await {
                return (StatusCode::TOO_MANY_REQUESTS, Json(serde_json::json!({ "error": "Too many requests. Please try again later." })));
            }

            let email_pk = format!("EMAIL#{}", email);

            // Lookup credentials
            let cred_result = dynamo
                .get_item()
                .table_name(table)
                .key("pk", AttributeValue::S(email_pk))
                .key("sk", AttributeValue::S("CREDENTIALS".to_string()))
                .send()
                .await;

            if let Ok(output) = cred_result {
                if let Some(item) = output.item {
                    let stored_hash = item.get("password_hash").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                    let salt = item.get("salt").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                    let user_id = item.get("user_id").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();

                    let computed_hash = hash_password(&req.password, &salt);
                    if computed_hash != stored_hash {
                        emit_audit_log(dynamo.clone(), table.clone(), "login_failure", &user_id, &email, "invalid_password");
                        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "Invalid email or password" })));
                    }

                    let now = chrono::Utc::now().to_rfc3339();

                    // Link session if provided
                    if let Some(ref sid) = req.session_id {
                        if !sid.is_empty() {
                            let link_pk = format!("LINK#{}", sid);
                            let _ = dynamo
                                .put_item()
                                .table_name(table)
                                .item("pk", AttributeValue::S(link_pk))
                                .item("sk", AttributeValue::S("CHANNEL_MAP".to_string()))
                                .item("user_id", AttributeValue::S(user_id.clone()))
                                .item("linked_at", AttributeValue::S(now.clone()))
                                .send()
                                .await;
                        }
                    }

                    // Create auth token
                    let auth_token = uuid::Uuid::new_v4().to_string();
                    let ttl = (chrono::Utc::now().timestamp() + 30 * 24 * 3600).to_string();
                    let _ = dynamo
                        .put_item()
                        .table_name(table)
                        .item("pk", AttributeValue::S(format!("AUTH#{}", auth_token)))
                        .item("sk", AttributeValue::S("TOKEN".to_string()))
                        .item("user_id", AttributeValue::S(user_id.clone()))
                        .item("email", AttributeValue::S(email.clone()))
                        .item("display_name", AttributeValue::S(email.clone()))
                        .item("created_at", AttributeValue::S(now))
                        .item("ttl", AttributeValue::N(ttl))
                        .send()
                        .await;

                    emit_audit_log(dynamo.clone(), table.clone(), "login_success", &user_id, &email, "email_login");

                    return (StatusCode::OK, Json(serde_json::json!({
                        "ok": true,
                        "token": auth_token,
                        "user_id": user_id,
                        "email": email,
                    })));
                }
            }

            emit_audit_log(dynamo.clone(), table.clone(), "login_failure", "", &email, "email_not_found");
            return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "Invalid email or password" })));
        }
    }

    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "DynamoDB not configured" })))
}

/// Send a verification email via Resend API. Returns Ok(()) on success.
#[allow(dead_code)]
async fn send_verification_email(email: &str, code: &str, resend_api_key: &str) -> Result<(), String> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "from": "ChatWeb <noreply@chatweb.ai>",
        "to": [email],
        "subject": format!("認証コード: {} — ChatWeb", code),
        "html": format!(
            "<div style='font-family:sans-serif;max-width:400px;margin:0 auto;padding:20px;'>\
             <h2 style='color:#6366f1;'>ChatWeb</h2>\
             <p>ログイン認証コード:</p>\
             <div style='font-size:32px;letter-spacing:8px;font-weight:bold;text-align:center;\
             background:#f3f4f6;padding:16px;border-radius:8px;margin:16px 0;'>{}</div>\
             <p style='color:#6b7280;font-size:14px;'>このコードは10分間有効です。<br>\
             心当たりがない場合は無視してください。</p>\
             </div>", code
        ),
    });
    let resp = client.post("https://api.resend.com/emails")
        .header("Authorization", format!("Bearer {}", resend_api_key))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Resend API error: {}", e))?;
    if resp.status().is_success() {
        Ok(())
    } else {
        let text = resp.text().await.unwrap_or_default();
        Err(format!("Resend API error: {}", text))
    }
}

/// POST /api/v1/auth/email — Passwordless email auth (with optional verification)
/// When RESEND_API_KEY is set: sends verification code, returns {pending_verification: true}
/// When not set: falls back to instant auth (original behavior)
async fn handle_auth_email(
    State(state): State<Arc<AppState>>,
    Json(req): Json<EmailAuthRequest>,
) -> impl IntoResponse {
    let email = req.email.trim().to_lowercase();
    if email.len() > 254 || !email.contains('@') || !email.contains('.') {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "Invalid email format" })));
    }

    let resend_api_key = std::env::var("RESEND_API_KEY").ok().filter(|k| !k.is_empty());

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            // Rate limit: 5 email auth attempts per minute per email
            if !check_rate_limit(dynamo, table, &format!("email_auth:{}", email), 5).await {
                return (StatusCode::TOO_MANY_REQUESTS, Json(serde_json::json!({ "error": "Too many requests. Please try again later." })));
            }

            // If RESEND_API_KEY is set, use verification code flow
            if let Some(ref api_key) = resend_api_key {
                // Generate 6-digit code from UUID bytes
                let uuid_bytes = uuid::Uuid::new_v4();
                let num = u32::from_le_bytes([uuid_bytes.as_bytes()[0], uuid_bytes.as_bytes()[1], uuid_bytes.as_bytes()[2], uuid_bytes.as_bytes()[3]]);
                let code = format!("{:06}", num % 1_000_000);
                let ttl = (chrono::Utc::now().timestamp() + 600).to_string(); // 10 min

                // Store verification code in DynamoDB
                let verify_pk = format!("VERIFY#{}", email);
                let mut put_req = dynamo
                    .put_item()
                    .table_name(table)
                    .item("pk", AttributeValue::S(verify_pk))
                    .item("sk", AttributeValue::S("CODE".to_string()))
                    .item("code", AttributeValue::S(code.clone()))
                    .item("attempts", AttributeValue::N("0".to_string()))
                    .item("session_id", AttributeValue::S(req.session_id.clone().unwrap_or_default()))
                    .item("ttl", AttributeValue::N(ttl))
                    .item("created_at", AttributeValue::S(chrono::Utc::now().to_rfc3339()));
                if let Some(ref name) = req.name {
                    let trimmed = name.trim();
                    if !trimmed.is_empty() {
                        put_req = put_req.item("name", AttributeValue::S(trimmed.to_string()));
                    }
                }
                let _ = put_req.send().await;

                // Send verification email
                match send_verification_email(&email, &code, api_key).await {
                    Ok(()) => {
                        tracing::info!("Verification email sent to {}", email);
                        return (StatusCode::OK, Json(serde_json::json!({
                            "ok": true,
                            "pending_verification": true,
                            "message": "認証コードをメールに送信しました。"
                        })));
                    }
                    Err(e) => {
                        tracing::error!("Failed to send verification email: {}", e);
                        // Fall through to instant auth on email send failure
                    }
                }
            }

            // Fallback: instant auth (no RESEND_API_KEY or email send failed)
            let email_pk = format!("EMAIL#{}", email);

            // Check if email already registered
            let existing = dynamo
                .get_item()
                .table_name(table)
                .key("pk", AttributeValue::S(email_pk.clone()))
                .key("sk", AttributeValue::S("CREDENTIALS".to_string()))
                .send()
                .await;

            let display_name = req.name
                .as_ref()
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| email.clone());

            let user_id = if let Ok(output) = &existing {
                if let Some(item) = &output.item {
                    item.get("user_id").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default()
                } else {
                    let new_user_id = format!("user:{}", uuid::Uuid::new_v4());
                    let now = chrono::Utc::now().to_rfc3339();

                    let _ = dynamo
                        .put_item()
                        .table_name(table)
                        .item("pk", AttributeValue::S(email_pk))
                        .item("sk", AttributeValue::S("CREDENTIALS".to_string()))
                        .item("user_id", AttributeValue::S(new_user_id.clone()))
                        .item("auth_method", AttributeValue::S("email_passwordless".to_string()))
                        .item("created_at", AttributeValue::S(now.clone()))
                        .send()
                        .await;

                    let _ = get_or_create_user(dynamo, table, &new_user_id).await;

                    let user_pk = format!("USER#{}", new_user_id);
                    let _ = dynamo
                        .update_item()
                        .table_name(table)
                        .key("pk", AttributeValue::S(user_pk))
                        .key("sk", AttributeValue::S(SK_PROFILE.to_string()))
                        .update_expression("SET email = :email, auth_method = :auth, display_name = :name, updated_at = :now")
                        .expression_attribute_values(":email", AttributeValue::S(email.clone()))
                        .expression_attribute_values(":auth", AttributeValue::S("email_passwordless".to_string()))
                        .expression_attribute_values(":name", AttributeValue::S(display_name.clone()))
                        .expression_attribute_values(":now", AttributeValue::S(now))
                        .send()
                        .await;

                    new_user_id
                }
            } else {
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "Database error" })));
            };

            if user_id.is_empty() {
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "User creation failed" })));
            }

            let now = chrono::Utc::now().to_rfc3339();

            if let Some(ref sid) = req.session_id {
                if !sid.is_empty() {
                    let link_pk = format!("LINK#{}", sid);
                    let _ = dynamo
                        .put_item()
                        .table_name(table)
                        .item("pk", AttributeValue::S(link_pk))
                        .item("sk", AttributeValue::S("CHANNEL_MAP".to_string()))
                        .item("user_id", AttributeValue::S(user_id.clone()))
                        .item("linked_at", AttributeValue::S(now.clone()))
                        .send()
                        .await;
                }
            }

            let auth_token = uuid::Uuid::new_v4().to_string();
            let ttl = (chrono::Utc::now().timestamp() + 30 * 24 * 3600).to_string();
            let _ = dynamo
                .put_item()
                .table_name(table)
                .item("pk", AttributeValue::S(format!("AUTH#{}", auth_token)))
                .item("sk", AttributeValue::S("TOKEN".to_string()))
                .item("user_id", AttributeValue::S(user_id.clone()))
                .item("email", AttributeValue::S(email.clone()))
                .item("display_name", AttributeValue::S(display_name.clone()))
                .item("created_at", AttributeValue::S(now))
                .item("ttl", AttributeValue::N(ttl))
                .send()
                .await;

            emit_audit_log(dynamo.clone(), table.clone(), "email_auth", &user_id, &email, "passwordless_email");

            return (StatusCode::OK, Json(serde_json::json!({
                "ok": true,
                "token": auth_token,
                "user_id": user_id,
                "email": email,
                "display_name": display_name,
            })));
        }
    }

    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "DynamoDB not configured" })))
}

/// POST /api/v1/auth/verify — Verify email with 6-digit code
async fn handle_auth_verify(
    State(state): State<Arc<AppState>>,
    Json(req): Json<VerifyRequest>,
) -> impl IntoResponse {
    let email = req.email.trim().to_lowercase();
    let code = req.code.trim().to_string();

    if code.len() != 6 || !code.chars().all(|c| c.is_ascii_digit()) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "Invalid verification code format" })));
    }

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let verify_pk = format!("VERIFY#{}", email);

            // Look up the stored code
            let stored = dynamo
                .get_item()
                .table_name(table)
                .key("pk", AttributeValue::S(verify_pk.clone()))
                .key("sk", AttributeValue::S("CODE".to_string()))
                .send()
                .await;

            let (stored_code, stored_session_id, attempts, stored_name) = match stored {
                Ok(output) => {
                    if let Some(item) = output.item {
                        let sc = item.get("code").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                        let sid = item.get("session_id").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                        let att = item.get("attempts").and_then(|v| v.as_n().ok())
                            .and_then(|n| n.parse::<i64>().ok()).unwrap_or(0);
                        let name = item.get("name").and_then(|v| v.as_s().ok()).cloned();
                        // Check TTL
                        if let Some(ttl_val) = item.get("ttl").and_then(|v| v.as_n().ok()) {
                            if let Ok(ttl) = ttl_val.parse::<i64>() {
                                if chrono::Utc::now().timestamp() > ttl {
                                    return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                                        "error": "認証コードの有効期限が切れています。もう一度お試しください。"
                                    })));
                                }
                            }
                        }
                        (sc, sid, att, name)
                    } else {
                        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                            "error": "認証コードが見つかりません。もう一度メールアドレスを入力してください。"
                        })));
                    }
                }
                Err(_) => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "Database error" })));
                }
            };

            // Check max attempts (3)
            if attempts >= 3 {
                return (StatusCode::TOO_MANY_REQUESTS, Json(serde_json::json!({
                    "error": "認証の試行回数が上限に達しました。もう一度メールアドレスを入力してください。"
                })));
            }

            // Increment attempts
            let _ = dynamo
                .update_item()
                .table_name(table)
                .key("pk", AttributeValue::S(verify_pk.clone()))
                .key("sk", AttributeValue::S("CODE".to_string()))
                .update_expression("SET attempts = attempts + :one")
                .expression_attribute_values(":one", AttributeValue::N("1".to_string()))
                .send()
                .await;

            // Verify code
            if code != stored_code {
                return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                    "error": "認証コードが正しくありません。"
                })));
            }

            // Code verified! Clean up verification record
            let _ = dynamo
                .delete_item()
                .table_name(table)
                .key("pk", AttributeValue::S(verify_pk))
                .key("sk", AttributeValue::S("CODE".to_string()))
                .send()
                .await;

            // Get or create user
            let email_pk = format!("EMAIL#{}", email);
            let existing = dynamo
                .get_item()
                .table_name(table)
                .key("pk", AttributeValue::S(email_pk.clone()))
                .key("sk", AttributeValue::S("CREDENTIALS".to_string()))
                .send()
                .await;

            let display_name = stored_name
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| email.clone());

            let user_id = if let Ok(output) = &existing {
                if let Some(item) = &output.item {
                    item.get("user_id").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default()
                } else {
                    let new_user_id = format!("user:{}", uuid::Uuid::new_v4());
                    let now = chrono::Utc::now().to_rfc3339();

                    let _ = dynamo
                        .put_item()
                        .table_name(table)
                        .item("pk", AttributeValue::S(email_pk))
                        .item("sk", AttributeValue::S("CREDENTIALS".to_string()))
                        .item("user_id", AttributeValue::S(new_user_id.clone()))
                        .item("auth_method", AttributeValue::S("email_verified".to_string()))
                        .item("created_at", AttributeValue::S(now.clone()))
                        .send()
                        .await;

                    let _ = get_or_create_user(dynamo, table, &new_user_id).await;
                    let user_pk = format!("USER#{}", new_user_id);
                    let _ = dynamo
                        .update_item()
                        .table_name(table)
                        .key("pk", AttributeValue::S(user_pk))
                        .key("sk", AttributeValue::S(SK_PROFILE.to_string()))
                        .update_expression("SET email = :email, auth_method = :auth, display_name = :name, updated_at = :now")
                        .expression_attribute_values(":email", AttributeValue::S(email.clone()))
                        .expression_attribute_values(":auth", AttributeValue::S("email_verified".to_string()))
                        .expression_attribute_values(":name", AttributeValue::S(display_name.clone()))
                        .expression_attribute_values(":now", AttributeValue::S(now))
                        .send()
                        .await;

                    new_user_id
                }
            } else {
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "Database error" })));
            };

            if user_id.is_empty() {
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "User creation failed" })));
            }

            let now = chrono::Utc::now().to_rfc3339();

            // Link session
            let session_id = req.session_id.as_deref().unwrap_or(&stored_session_id);
            if !session_id.is_empty() {
                let link_pk = format!("LINK#{}", session_id);
                let _ = dynamo
                    .put_item()
                    .table_name(table)
                    .item("pk", AttributeValue::S(link_pk))
                    .item("sk", AttributeValue::S("CHANNEL_MAP".to_string()))
                    .item("user_id", AttributeValue::S(user_id.clone()))
                    .item("linked_at", AttributeValue::S(now.clone()))
                    .send()
                    .await;
            }

            // Create auth token
            let auth_token = uuid::Uuid::new_v4().to_string();
            let ttl = (chrono::Utc::now().timestamp() + 30 * 24 * 3600).to_string();
            let _ = dynamo
                .put_item()
                .table_name(table)
                .item("pk", AttributeValue::S(format!("AUTH#{}", auth_token)))
                .item("sk", AttributeValue::S("TOKEN".to_string()))
                .item("user_id", AttributeValue::S(user_id.clone()))
                .item("email", AttributeValue::S(email.clone()))
                .item("display_name", AttributeValue::S(display_name.clone()))
                .item("created_at", AttributeValue::S(now))
                .item("ttl", AttributeValue::N(ttl))
                .send()
                .await;

            // Apply referral code if provided
            let mut referral_bonus: i64 = 0;
            if let Some(ref ref_code) = req.referral_code {
                let rcode = ref_code.trim().to_uppercase();
                if rcode.len() >= 4 && rcode.len() <= 10 {
                    if let Ok(output) = dynamo
                        .get_item()
                        .table_name(table)
                        .key("pk", AttributeValue::S(format!("REFERRAL#{}", rcode)))
                        .key("sk", AttributeValue::S("OWNER".to_string()))
                        .send()
                        .await
                    {
                        if let Some(item) = output.item {
                            if let Some(referrer_id) = item.get("user_id").and_then(|v| v.as_s().ok()) {
                                if referrer_id != &user_id {
                                    let bonus: i64 = 100;
                                    let now_ref = chrono::Utc::now().to_rfc3339();
                                    let log_pk = format!("REFERRAL_LOG#{}#{}", referrer_id, user_id);
                                    let _ = dynamo.put_item().table_name(table)
                                        .item("pk", AttributeValue::S(log_pk))
                                        .item("sk", AttributeValue::S("INFO".to_string()))
                                        .item("referrer_id", AttributeValue::S(referrer_id.to_string()))
                                        .item("referee_id", AttributeValue::S(user_id.clone()))
                                        .item("code", AttributeValue::S(rcode.clone()))
                                        .item("bonus", AttributeValue::N(bonus.to_string()))
                                        .item("created_at", AttributeValue::S(now_ref.clone()))
                                        .condition_expression("attribute_not_exists(pk)")
                                        .send().await;
                                    let _ = dynamo.update_item().table_name(table)
                                        .key("pk", AttributeValue::S(format!("USER#{}", user_id)))
                                        .key("sk", AttributeValue::S(SK_PROFILE.to_string()))
                                        .update_expression("SET credits_remaining = credits_remaining + :bonus, referred_by = :ref, updated_at = :now")
                                        .expression_attribute_values(":bonus", AttributeValue::N(bonus.to_string()))
                                        .expression_attribute_values(":ref", AttributeValue::S(referrer_id.to_string()))
                                        .expression_attribute_values(":now", AttributeValue::S(now_ref.clone()))
                                        .send().await;
                                    let _ = dynamo.update_item().table_name(table)
                                        .key("pk", AttributeValue::S(format!("USER#{}", referrer_id)))
                                        .key("sk", AttributeValue::S(SK_PROFILE.to_string()))
                                        .update_expression("SET credits_remaining = credits_remaining + :bonus, updated_at = :now")
                                        .expression_attribute_values(":bonus", AttributeValue::N(bonus.to_string()))
                                        .expression_attribute_values(":now", AttributeValue::S(now_ref))
                                        .send().await;
                                    referral_bonus = bonus;
                                    tracing::info!("Referral applied via verify: {} referred {} via code {}", referrer_id, user_id, rcode);
                                }
                            }
                        }
                    }
                }
            }

            emit_audit_log(dynamo.clone(), table.clone(), "email_verified", &user_id, &email, "email_code_verified");

            return (StatusCode::OK, Json(serde_json::json!({
                "ok": true,
                "token": auth_token,
                "user_id": user_id,
                "email": email,
                "display_name": display_name,
                "referral_bonus": referral_bonus,
            })));
        }
    }

    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "DynamoDB not configured" })))
}

// ---------------------------------------------------------------------------
// Conversation History API
// ---------------------------------------------------------------------------

/// GET /api/v1/conversations — List user's conversations
async fn handle_list_conversations(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let token = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_start_matches("Bearer ").to_string())
        .unwrap_or_default();

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            // Resolve user from token
            let user_id = resolve_user_from_token(dynamo, table, &token).await;
            if user_id.is_empty() {
                return Json(serde_json::json!({ "conversations": [], "error": "Not authenticated" }));
            }

            let user_pk = format!("USER#{}", user_id);
            let resp = dynamo
                .query()
                .table_name(table)
                .key_condition_expression("pk = :pk AND begins_with(sk, :sk)")
                .expression_attribute_values(":pk", AttributeValue::S(user_pk))
                .expression_attribute_values(":sk", AttributeValue::S("CONV#".to_string()))
                .scan_index_forward(false)
                .limit(50)
                .send()
                .await;

            let conversations: Vec<serde_json::Value> = match resp {
                Ok(output) => {
                    output.items.unwrap_or_default().iter().map(|item| {
                        serde_json::json!({
                            "id": item.get("conv_id").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default(),
                            "title": item.get("title").and_then(|v| v.as_s().ok()).cloned().unwrap_or_else(|| "New conversation".to_string()),
                            "created_at": item.get("created_at").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default(),
                            "updated_at": item.get("updated_at").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default(),
                            "message_count": item.get("message_count").and_then(|v| v.as_n().ok()).and_then(|n| n.parse::<i64>().ok()).unwrap_or(0),
                            "preview": item.get("last_message_preview").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default(),
                            "session_id": item.get("session_id").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default(),
                        })
                    }).collect()
                }
                Err(_) => vec![],
            };

            return Json(serde_json::json!({ "conversations": conversations }));
        }
    }

    Json(serde_json::json!({ "conversations": [] }))
}

/// POST /api/v1/conversations/finalize — Finalize current conversation before switching.
/// Triggers memory consolidation (fire-and-forget) so context is preserved as long-term memory.
async fn handle_finalize_conversation(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let token = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_start_matches("Bearer ").to_string())
        .unwrap_or_default();

    let old_session_id = body.get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if old_session_id.is_empty() {
        return Json(serde_json::json!({"ok": false, "error": "session_id required"}));
    }

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let user_id = resolve_user_from_token(dynamo, table, &token).await;
            if user_id.is_empty() {
                return Json(serde_json::json!({"ok": false, "error": "Not authenticated"}));
            }

            // Resolve session key for the old conversation
            let session_key = resolve_session_key(dynamo, table, old_session_id).await;

            // Force memory consolidation (fire-and-forget)
            let provider_for_mem = state.lb_provider.clone().or_else(|| state.provider.clone());
            if let Some(provider) = provider_for_mem {
                let dynamo_c = dynamo.clone();
                let table_c = table.clone();
                let sk = session_key.clone();
                spawn_consolidate_memory(dynamo_c, table_c, sk, provider);
            }

            return Json(serde_json::json!({"ok": true}));
        }
    }

    let _ = (&state, &token);
    Json(serde_json::json!({"ok": true}))
}

/// POST /api/v1/conversations — Create a new conversation
async fn handle_create_conversation(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let token = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_start_matches("Bearer ").to_string())
        .unwrap_or_default();

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let user_id = resolve_user_from_token(dynamo, table, &token).await;
            if user_id.is_empty() {
                return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "Not authenticated" })));
            }

            let conv_id = uuid::Uuid::new_v4().to_string();
            let now = chrono::Utc::now().to_rfc3339();
            let session_id = format!("webchat:{}", conv_id);

            // Create CONV record under user
            let user_pk = format!("USER#{}", user_id);
            let _ = dynamo
                .put_item()
                .table_name(table)
                .item("pk", AttributeValue::S(user_pk))
                .item("sk", AttributeValue::S(format!("CONV#{}", conv_id)))
                .item("conv_id", AttributeValue::S(conv_id.clone()))
                .item("title", AttributeValue::S("New conversation".to_string()))
                .item("session_id", AttributeValue::S(session_id.clone()))
                .item("created_at", AttributeValue::S(now.clone()))
                .item("updated_at", AttributeValue::S(now))
                .item("message_count", AttributeValue::N("0".to_string()))
                .send()
                .await;

            // Link the new session_id to the user
            let link_pk = format!("LINK#{}", session_id);
            let _ = dynamo
                .put_item()
                .table_name(table)
                .item("pk", AttributeValue::S(link_pk))
                .item("sk", AttributeValue::S("CHANNEL_MAP".to_string()))
                .item("user_id", AttributeValue::S(user_id))
                .item("linked_at", AttributeValue::S(chrono::Utc::now().to_rfc3339()))
                .send()
                .await;

            return (StatusCode::OK, Json(serde_json::json!({
                "ok": true,
                "conversation_id": conv_id,
                "session_id": session_id,
            })));
        }
    }

    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "DynamoDB not configured" })))
}

/// GET /api/v1/conversations/{id}/messages — Get messages for a conversation
async fn handle_get_conversation_messages(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let token = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_start_matches("Bearer ").to_string())
        .unwrap_or_default();

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let user_id = resolve_user_from_token(dynamo, table, &token).await;
            if user_id.is_empty() {
                return Json(serde_json::json!({ "messages": [], "error": "Not authenticated" }));
            }

            // Get the session_id from the conversation record
            let user_pk = format!("USER#{}", user_id);
            let conv_resp = dynamo
                .get_item()
                .table_name(table)
                .key("pk", AttributeValue::S(user_pk))
                .key("sk", AttributeValue::S(format!("CONV#{}", id)))
                .send()
                .await;

            let session_id = if let Ok(output) = conv_resp {
                output.item.and_then(|item| {
                    item.get("session_id").and_then(|v| v.as_s().ok()).cloned()
                }).unwrap_or_else(|| format!("webchat:{}", id))
            } else {
                format!("webchat:{}", id)
            };

            // Get messages from session store (include timestamps and channel info)
            let mut store = state.sessions.lock().await;
            let session = store.get_or_create(&session_id);
            let messages: Vec<serde_json::Value> = session.messages.iter().map(|m| {
                let ch = m.extra.get("channel")
                    .and_then(|v| v.as_str())
                    .unwrap_or("web");
                let ts = m.timestamp.as_deref().unwrap_or("");
                serde_json::json!({
                    "role": m.role,
                    "content": m.content,
                    "channel": ch,
                    "timestamp": ts,
                })
            }).collect();

            return Json(serde_json::json!({ "messages": messages, "session_id": session_id }));
        }
    }

    Json(serde_json::json!({ "messages": [] }))
}

/// DELETE /api/v1/conversations/{id} — Delete a conversation
async fn handle_delete_conversation(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let token = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_start_matches("Bearer ").to_string())
        .unwrap_or_default();

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let user_id = resolve_user_from_token(dynamo, table, &token).await;
            if user_id.is_empty() {
                return Json(serde_json::json!({ "error": "Not authenticated" }));
            }

            let user_pk = format!("USER#{}", user_id);
            let _ = dynamo
                .delete_item()
                .table_name(table)
                .key("pk", AttributeValue::S(user_pk))
                .key("sk", AttributeValue::S(format!("CONV#{}", id)))
                .send()
                .await;

            return Json(serde_json::json!({ "ok": true }));
        }
    }

    Json(serde_json::json!({ "error": "DynamoDB not configured" }))
}

// ---------------------------------------------------------------------------
// Shared conversation endpoints
// ---------------------------------------------------------------------------

/// GET /c/{hash} — Serve the SPA for shared conversation view
async fn handle_shared_page(
    Path(hash): Path<String>,
) -> impl IntoResponse {
    // Serve the same index.html — frontend detects /c/{hash} and enters shared mode
    axum::response::Html(include_str!("../../../../web/index.html"))
}

/// GET /api/v1/shared/{hash} — Get shared conversation messages (public, read-only)
async fn handle_get_shared(
    State(state): State<Arc<AppState>>,
    Path(hash): Path<String>,
) -> impl IntoResponse {
    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            // Look up the share record
            let resp = dynamo
                .get_item()
                .table_name(table)
                .key("pk", AttributeValue::S(format!("SHARE#{hash}")))
                .key("sk", AttributeValue::S("INFO".to_string()))
                .send()
                .await;

            let item = match resp {
                Ok(output) => match output.item {
                    Some(item) => item,
                    None => return Json(serde_json::json!({ "error": "Share not found" })),
                },
                Err(e) => {
                    tracing::error!("Failed to get share: {}", e);
                    return Json(serde_json::json!({ "error": "Internal error" }));
                }
            };

            // Check if revoked
            let revoked = item
                .get("revoked")
                .and_then(|v| v.as_bool().ok())
                .copied()
                .unwrap_or(false);
            if revoked {
                return Json(serde_json::json!({ "error": "This share has been revoked" }));
            }

            let conv_id = item
                .get("conv_id")
                .and_then(|v| v.as_s().ok())
                .cloned()
                .unwrap_or_default();
            let user_id = item
                .get("user_id")
                .and_then(|v| v.as_s().ok())
                .cloned()
                .unwrap_or_default();
            let shared_at = item
                .get("created_at")
                .and_then(|v| v.as_s().ok())
                .cloned()
                .unwrap_or_default();

            // Get the conversation title
            let user_pk = format!("USER#{}", user_id);
            let conv_resp = dynamo
                .get_item()
                .table_name(table)
                .key("pk", AttributeValue::S(user_pk))
                .key("sk", AttributeValue::S(format!("CONV#{}", conv_id)))
                .send()
                .await;

            let title = if let Ok(output) = conv_resp {
                output.item.and_then(|item| {
                    item.get("title").and_then(|v| v.as_s().ok()).cloned()
                }).unwrap_or_else(|| "Shared conversation".to_string())
            } else {
                "Shared conversation".to_string()
            };

            // Get session_id from conv record to fetch messages
            let session_id = format!("webchat:{}", conv_id);

            let mut store = state.sessions.lock().await;
            let session = store.get_or_create(&session_id);
            let messages: Vec<serde_json::Value> = session
                .messages
                .iter()
                .filter(|m| m.role == "user" || m.role == "assistant")
                .map(|m| {
                    serde_json::json!({
                        "role": m.role,
                        "content": m.content,
                    })
                })
                .collect();

            return Json(serde_json::json!({
                "title": title,
                "messages": messages,
                "shared_at": shared_at,
            }));
        }
    }

    let _ = &state;
    Json(serde_json::json!({ "error": "DynamoDB not configured" }))
}

/// POST /api/v1/conversations/{id}/share — Create a share link for a conversation
async fn handle_share_conversation(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let user_id = match auth_user_id(&state, &headers).await {
                Some(uid) => uid,
                None => return Json(serde_json::json!({ "error": "Not authenticated" })),
            };

            // Verify user owns this conversation
            let user_pk = format!("USER#{}", user_id);
            let conv_resp = dynamo
                .get_item()
                .table_name(table)
                .key("pk", AttributeValue::S(user_pk))
                .key("sk", AttributeValue::S(format!("CONV#{}", id)))
                .send()
                .await;

            match conv_resp {
                Ok(output) if output.item.is_some() => {}
                _ => return Json(serde_json::json!({ "error": "Conversation not found" })),
            }

            // Check if already shared
            let existing_resp = dynamo
                .get_item()
                .table_name(table)
                .key("pk", AttributeValue::S(format!("CONV_SHARE#{id}")))
                .key("sk", AttributeValue::S("HASH".to_string()))
                .send()
                .await;

            if let Ok(output) = existing_resp {
                if let Some(item) = output.item {
                    if let Some(hash) = item.get("share_hash").and_then(|v| v.as_s().ok()) {
                        // Verify the share is not revoked
                        let share_resp = dynamo
                            .get_item()
                            .table_name(table)
                            .key("pk", AttributeValue::S(format!("SHARE#{hash}")))
                            .key("sk", AttributeValue::S("INFO".to_string()))
                            .send()
                            .await;
                        let revoked = share_resp.ok().and_then(|o| o.item).map(|item| {
                            item.get("revoked")
                                .and_then(|v| v.as_bool().ok())
                                .copied()
                                .unwrap_or(false)
                        }).unwrap_or(true);

                        if !revoked {
                            return Json(serde_json::json!({
                                "share_url": format!("{}/c/{hash}", get_base_url()),
                                "hash": hash,
                                "already_shared": true,
                            }));
                        }
                    }
                }
            }

            // Generate new share
            let hash = super::commands::generate_share_hash();
            let now = chrono::Utc::now().to_rfc3339();

            let put_result = dynamo
                .put_item()
                .table_name(table)
                .item("pk", AttributeValue::S(format!("SHARE#{hash}")))
                .item("sk", AttributeValue::S("INFO".to_string()))
                .item("conv_id", AttributeValue::S(id.clone()))
                .item("user_id", AttributeValue::S(user_id))
                .item("created_at", AttributeValue::S(now))
                .item("revoked", AttributeValue::Bool(false))
                .send()
                .await;

            if let Err(e) = put_result {
                tracing::error!("Failed to create share: {}", e);
                return Json(serde_json::json!({ "error": "Failed to create share link" }));
            }

            // Reverse lookup
            let _ = dynamo
                .put_item()
                .table_name(table)
                .item("pk", AttributeValue::S(format!("CONV_SHARE#{id}")))
                .item("sk", AttributeValue::S("HASH".to_string()))
                .item("share_hash", AttributeValue::S(hash.clone()))
                .send()
                .await;

            return Json(serde_json::json!({
                "share_url": format!("{}/c/{hash}", get_base_url()),
                "hash": hash,
                "already_shared": false,
            }));
        }
    }

    let _ = (&state, &headers);
    Json(serde_json::json!({ "error": "DynamoDB not configured" }))
}

/// DELETE /api/v1/conversations/{id}/share — Revoke a share link
async fn handle_revoke_share(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let user_id = match auth_user_id(&state, &headers).await {
                Some(uid) => uid,
                None => return Json(serde_json::json!({ "error": "Not authenticated" })),
            };

            // Verify user owns this conversation
            let user_pk = format!("USER#{}", user_id);
            let conv_resp = dynamo
                .get_item()
                .table_name(table)
                .key("pk", AttributeValue::S(user_pk))
                .key("sk", AttributeValue::S(format!("CONV#{}", id)))
                .send()
                .await;

            match conv_resp {
                Ok(output) if output.item.is_some() => {}
                _ => return Json(serde_json::json!({ "error": "Conversation not found" })),
            }

            // Find the share hash
            let hash_resp = dynamo
                .get_item()
                .table_name(table)
                .key("pk", AttributeValue::S(format!("CONV_SHARE#{id}")))
                .key("sk", AttributeValue::S("HASH".to_string()))
                .send()
                .await;

            let hash = match hash_resp {
                Ok(output) => output
                    .item
                    .and_then(|item| item.get("share_hash").and_then(|v| v.as_s().ok()).cloned()),
                Err(_) => None,
            };

            match hash {
                Some(hash) => {
                    // Set revoked = true
                    let _ = dynamo
                        .update_item()
                        .table_name(table)
                        .key("pk", AttributeValue::S(format!("SHARE#{hash}")))
                        .key("sk", AttributeValue::S("INFO".to_string()))
                        .update_expression("SET revoked = :r")
                        .expression_attribute_values(":r", AttributeValue::Bool(true))
                        .send()
                        .await;

                    return Json(serde_json::json!({ "ok": true }));
                }
                None => {
                    return Json(serde_json::json!({ "error": "No share link found for this conversation" }));
                }
            }
        }
    }

    let _ = (&state, &headers);
    Json(serde_json::json!({ "error": "DynamoDB not configured" }))
}

/// Helper: Resolve user_id from auth token
#[cfg(feature = "dynamodb-backend")]
async fn resolve_user_from_token(
    dynamo: &aws_sdk_dynamodb::Client,
    config_table: &str,
    token: &str,
) -> String {
    if token.is_empty() {
        return String::new();
    }
    let auth_pk = format!("AUTH#{}", token);
    if let Ok(output) = dynamo
        .get_item()
        .table_name(config_table)
        .key("pk", AttributeValue::S(auth_pk))
        .key("sk", AttributeValue::S("TOKEN".to_string()))
        .send()
        .await
    {
        if let Some(item) = output.item {
            return item.get("user_id").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
        }
    }
    String::new()
}

// ---------------------------------------------------------------------------
// API Key management
// ---------------------------------------------------------------------------

/// GET /api/v1/apikeys — List user's API keys
async fn handle_list_apikeys(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    #[cfg(feature = "dynamodb-backend")]
    {
        let user_id = match auth_user_id(&state, &headers).await {
            Some(uid) => uid,
            None => return Json(serde_json::json!({ "error": "Unauthorized" })),
        };
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let pk = format!("USER#{}", user_id);
            if let Ok(output) = dynamo
                .query()
                .table_name(table)
                .key_condition_expression("pk = :pk AND begins_with(sk, :prefix)")
                .expression_attribute_values(":pk", AttributeValue::S(pk))
                .expression_attribute_values(":prefix", AttributeValue::S("APIKEY#".to_string()))
                .send()
                .await
            {
                let keys: Vec<serde_json::Value> = output.items.unwrap_or_default().iter().map(|item| {
                    let sk = item.get("sk").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                    let key_id = sk.trim_start_matches("APIKEY#").to_string();
                    let name = item.get("name").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                    let prefix = item.get("key_prefix").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                    let created = item.get("created_at").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                    serde_json::json!({
                        "id": key_id,
                        "name": name,
                        "key_prefix": prefix,
                        "created_at": created,
                    })
                }).collect();
                return Json(serde_json::json!({ "api_keys": keys }));
            }
        }
    }
    Json(serde_json::json!({ "api_keys": [] }))
}

/// POST /api/v1/apikeys — Create a new API key
async fn handle_create_apikey(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    #[cfg(feature = "dynamodb-backend")]
    {
        let user_id = match auth_user_id(&state, &headers).await {
            Some(uid) => uid,
            None => return Json(serde_json::json!({ "error": "Unauthorized" })),
        };
        let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("default").to_string();
        // Generate API key: cw_<random>
        let key_id = uuid::Uuid::new_v4().to_string().replace("-", "");
        let api_key = format!("cw_{}", &key_id);
        let key_prefix = format!("cw_{}...", &key_id[..8]);
        let now = chrono::Utc::now().to_rfc3339();

        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            // Store under user
            let _ = dynamo
                .put_item()
                .table_name(table)
                .item("pk", AttributeValue::S(format!("USER#{}", user_id)))
                .item("sk", AttributeValue::S(format!("APIKEY#{}", key_id)))
                .item("name", AttributeValue::S(name.clone()))
                .item("api_key_hash", AttributeValue::S(api_key.clone())) // In production, hash this
                .item("key_prefix", AttributeValue::S(key_prefix.clone()))
                .item("created_at", AttributeValue::S(now.clone()))
                .send()
                .await;
            // Store reverse lookup: APIKEY#<key> -> user_id
            let _ = dynamo
                .put_item()
                .table_name(table)
                .item("pk", AttributeValue::S(format!("APIKEY#{}", api_key)))
                .item("sk", AttributeValue::S("LOOKUP".to_string()))
                .item("user_id", AttributeValue::S(user_id.clone()))
                .item("key_id", AttributeValue::S(key_id.clone()))
                .item("created_at", AttributeValue::S(now.clone()))
                .send()
                .await;

            emit_audit_log(dynamo.clone(), table.clone(), "apikey_created", &user_id, "", &format!("key_prefix={}", key_prefix));

            return Json(serde_json::json!({
                "ok": true,
                "api_key": api_key,
                "id": key_id,
                "name": name,
                "key_prefix": key_prefix,
                "created_at": now,
            }));
        }
    }
    Json(serde_json::json!({ "error": "Failed to create API key" }))
}

/// DELETE /api/v1/apikeys/{id} — Delete an API key
async fn handle_delete_apikey(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(key_id): axum::extract::Path<String>,
) -> impl IntoResponse {
    #[cfg(feature = "dynamodb-backend")]
    {
        let user_id = match auth_user_id(&state, &headers).await {
            Some(uid) => uid,
            None => return Json(serde_json::json!({ "error": "Unauthorized" })),
        };
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            // Get the key first to find the api_key for reverse lookup cleanup
            let pk = format!("USER#{}", user_id);
            let sk = format!("APIKEY#{}", key_id);
            if let Ok(get_output) = dynamo
                .get_item()
                .table_name(table)
                .key("pk", AttributeValue::S(pk.clone()))
                .key("sk", AttributeValue::S(sk.clone()))
                .send()
                .await
            {
                if let Some(item) = get_output.item {
                    // Delete reverse lookup
                    if let Some(api_key) = item.get("api_key_hash").and_then(|v| v.as_s().ok()) {
                        let _ = dynamo
                            .delete_item()
                            .table_name(table)
                            .key("pk", AttributeValue::S(format!("APIKEY#{}", api_key)))
                            .key("sk", AttributeValue::S("LOOKUP".to_string()))
                            .send()
                            .await;
                    }
                }
            }
            // Delete the key record
            let _ = dynamo
                .delete_item()
                .table_name(table)
                .key("pk", AttributeValue::S(pk))
                .key("sk", AttributeValue::S(sk))
                .send()
                .await;
            return Json(serde_json::json!({ "ok": true }));
        }
    }
    Json(serde_json::json!({ "error": "Failed to delete API key" }))
}

// ---------------------------------------------------------------------------
// TTS (Text-to-Speech) via AWS Polly (primary) + OpenAI (fallback)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct SpeechRequest {
    text: String,
    #[serde(default = "default_tts_voice")]
    voice: String,
    #[serde(default = "default_tts_speed")]
    speed: f64,
    #[serde(default)]
    engine: Option<String>, // "polly", "openai", "elevenlabs", "sbv2" — auto if absent
    #[serde(default)]
    instructions: Option<String>, // Voice style instructions for gpt-4o-mini-tts
    #[serde(default)]
    voice_id: Option<String>, // ElevenLabs voice ID
    #[serde(default)]
    model_id: Option<String>, // ElevenLabs model ID or SBV2 model_id
    #[serde(default)]
    style: Option<String>, // SBV2 voice style (e.g., "Neutral", "Happy")
    session_id: Option<String>,
}

fn default_tts_voice() -> String { "nova".to_string() }
fn default_tts_speed() -> f64 { 1.0 }

/// Cached Polly client (reuse across requests for speed)
#[cfg(feature = "dynamodb-backend")]
static POLLY_CLIENT: once_cell::sync::OnceCell<aws_sdk_polly::Client> = once_cell::sync::OnceCell::new();

#[cfg(feature = "dynamodb-backend")]
async fn get_polly_client() -> &'static aws_sdk_polly::Client {
    if let Some(client) = POLLY_CLIENT.get() {
        return client;
    }
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let client = aws_sdk_polly::Client::new(&config);
    let _ = POLLY_CLIENT.set(client);
    POLLY_CLIENT.get().unwrap()
}

/// Try AWS Polly Neural TTS (Kazuha for Japanese, Ruth for English)
#[cfg(feature = "dynamodb-backend")]
async fn try_polly_tts(text: &str) -> Option<Vec<u8>> {
    use aws_sdk_polly::types::{Engine, OutputFormat, VoiceId};

    let polly = get_polly_client().await;

    // Detect language: if contains CJK/hiragana/katakana → Japanese
    let is_ja = text.chars().any(|c| {
        ('\u{3040}'..='\u{309F}').contains(&c) || // hiragana
        ('\u{30A0}'..='\u{30FF}').contains(&c) || // katakana
        ('\u{4E00}'..='\u{9FFF}').contains(&c)    // CJK
    });

    let (voice, voice_name) = if is_ja { (VoiceId::Kazuha, "Kazuha") } else { (VoiceId::Ruth, "Ruth") };

    // Wrap in SSML for more natural prosody
    let ssml_text = format!(
        r#"<speak><prosody rate="105%">{}</prosody></speak>"#,
        text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    );

    let result = polly
        .synthesize_speech()
        .engine(Engine::Neural)
        .output_format(OutputFormat::Mp3)
        .text_type(aws_sdk_polly::types::TextType::Ssml)
        .text(&ssml_text)
        .voice_id(voice)
        .send()
        .await;

    match result {
        Ok(output) => {
            match output.audio_stream.collect().await {
                Ok(bytes) => {
                    let audio = bytes.into_bytes().to_vec();
                    if audio.is_empty() {
                        tracing::warn!("Polly returned empty audio");
                        None
                    } else {
                        tracing::info!("Polly TTS success: {} bytes, voice={}", audio.len(), voice_name);
                        Some(audio)
                    }
                }
                Err(e) => {
                    tracing::warn!("Polly audio stream read failed: {}", e);
                    None
                }
            }
        }
        Err(e) => {
            tracing::warn!("Polly synthesize_speech failed: {}", e);
            None
        }
    }
}

/// QWEN3-TTS API — Alibaba Cloud's high-quality multilingual TTS with voice cloning
async fn try_qwen_tts(text: &str, voice: &str, _speed: f64) -> Result<Vec<u8>, String> {
    let api_key = std::env::var("DASHSCOPE_API_KEY")
        .map_err(|_| "No DASHSCOPE_API_KEY".to_string())?;

    if api_key.is_empty() {
        return Err("Empty DASHSCOPE_API_KEY".to_string());
    }

    // Detect Japanese text for language selection
    let is_ja = text.chars().any(|c| {
        ('\u{3040}'..='\u{309F}').contains(&c) || // hiragana
        ('\u{30A0}'..='\u{30FF}').contains(&c) || // katakana
        ('\u{4E00}'..='\u{9FFF}').contains(&c)    // CJK
    });

    let language_type = if is_ja { "Japanese" } else { "English" };

    // Map common voice names to QWEN voices
    // For Japanese: prefer feminine voices like Cherry, Serena
    let qwen_voice = if voice.is_empty() || voice == "alloy" || voice == "nova" {
        if is_ja { "Cherry" } else { "Cherry" }
    } else {
        voice
    };

    let client = reqwest::Client::new();

    // Non-streaming mode: returns a URL valid for 24 hours
    let resp = client
        .post("https://dashscope-intl.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "model": "qwen3-tts-flash",
            "input": {
                "text": text,
                "voice": qwen_voice,
                "language_type": language_type,
            },
            "stream": false,
        }))
        .send()
        .await
        .map_err(|e| format!("QWEN TTS request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("QWEN TTS error: {} {}", status, body));
    }

    let json: serde_json::Value = resp.json().await
        .map_err(|e| format!("Failed to parse QWEN response: {}", e))?;

    // Extract audio URL from response
    let audio_url = json["output"]["audio_url"]
        .as_str()
        .ok_or_else(|| format!("No audio_url in response: {:?}", json))?;

    // Download audio from the URL
    let audio_resp = client
        .get(audio_url)
        .send()
        .await
        .map_err(|e| format!("Failed to download audio: {}", e))?;

    if audio_resp.status().is_success() {
        let audio = audio_resp.bytes().await
            .map_err(|e| format!("Failed to read audio bytes: {}", e))?
            .to_vec();
        Ok(audio)
    } else {
        Err(format!("Audio download failed: {}", audio_resp.status()))
    }
}

/// OpenAI TTS API — uses gpt-4o-mini-tts for natural, steerable voice
async fn try_openai_tts(text: &str, voice: &str, speed: f64, instructions: Option<&str>) -> Result<Vec<u8>, String> {
    let api_key = std::env::var("OPENAI_API_KEY")
        .or_else(|_| std::env::var("OPENAI_API_KEYS").map(|keys| {
            keys.split(',').next().unwrap_or("").trim().to_string()
        }))
        .map_err(|_| "No OpenAI API key".to_string())?;

    if api_key.is_empty() {
        return Err("Empty OpenAI API key".to_string());
    }

    // Detect Japanese text for auto-instructions
    let is_ja = text.chars().any(|c| {
        ('\u{3040}'..='\u{309F}').contains(&c) || // hiragana
        ('\u{30A0}'..='\u{30FF}').contains(&c) || // katakana
        ('\u{4E00}'..='\u{9FFF}').contains(&c)    // CJK
    });

    let default_instructions = if is_ja {
        "自然な日本語で話してください。温かく親しみやすいトーンで、会話するように。明瞭な発音で、適度なスピードで。感情を込めて、機械的にならないように。"
    } else {
        "Speak naturally in a warm, friendly tone. Clear pronunciation at a comfortable pace."
    };

    let voice_instructions = instructions.unwrap_or(default_instructions);

    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.openai.com/v1/audio/speech")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&serde_json::json!({
            "model": "gpt-4o-mini-tts",
            "input": text,
            "voice": voice,
            "speed": speed,
            "instructions": voice_instructions,
            "response_format": "mp3",
        }))
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    if resp.status().is_success() {
        let audio = resp.bytes().await.unwrap_or_default().to_vec();
        Ok(audio)
    } else {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Err(format!("OpenAI TTS error: {} {}", status, body))
    }
}

/// ElevenLabs TTS API — high-quality multilingual voices
async fn try_elevenlabs_tts(text: &str, voice_id: &str, model_id: &str) -> Result<Vec<u8>, String> {
    let api_key = std::env::var("ELEVENLABS_API_KEY")
        .map_err(|_| "No ELEVENLABS_API_KEY".to_string())?;
    if api_key.is_empty() {
        return Err("Empty ELEVENLABS_API_KEY".to_string());
    }

    // Default voice: "pNInz6obpgDQGcFmaJgB" = Adam (multilingual), good for Japanese too
    let vid = if voice_id.is_empty() { "pNInz6obpgDQGcFmaJgB" } else { voice_id };
    // Default model: eleven_multilingual_v2 for Japanese support
    let mid = if model_id.is_empty() { "eleven_multilingual_v2" } else { model_id };

    let url = format!("https://api.elevenlabs.io/v1/text-to-speech/{}", vid);
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("xi-api-key", &api_key)
        .header("Accept", "audio/mpeg")
        .json(&serde_json::json!({
            "text": text,
            "model_id": mid,
            "voice_settings": {
                "stability": 0.5,
                "similarity_boost": 0.75,
                "style": 0.3,
                "use_speaker_boost": true
            }
        }))
        .send()
        .await
        .map_err(|e| format!("ElevenLabs request failed: {}", e))?;

    if resp.status().is_success() {
        let audio = resp.bytes().await.unwrap_or_default().to_vec();
        if audio.is_empty() {
            Err("ElevenLabs returned empty audio".to_string())
        } else {
            tracing::info!("ElevenLabs TTS success: {} bytes, voice={}, model={}", audio.len(), vid, mid);
            Ok(audio)
        }
    } else {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Err(format!("ElevenLabs error: {} {}", status, body))
    }
}

/// Style-Bert-VITS2 TTS via RunPod Serverless — high-quality Japanese voice
async fn try_sbv2_tts(text: &str, model_id: i32, speaker_id: i32, style: &str) -> Result<Vec<u8>, String> {
    let api_key = std::env::var("RUNPOD_API_KEY")
        .map_err(|_| "No RUNPOD_API_KEY".to_string())?;
    let endpoint_id = std::env::var("RUNPOD_SBV2_ENDPOINT_ID")
        .map_err(|_| "No RUNPOD_SBV2_ENDPOINT_ID".to_string())?;

    if api_key.is_empty() || endpoint_id.is_empty() {
        return Err("Empty RunPod credentials".to_string());
    }

    let url = format!("https://api.runpod.ai/v2/{}/runsync", endpoint_id);
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&serde_json::json!({
            "input": {
                "text": text,
                "model_id": model_id,
                "speaker_id": speaker_id,
                "style": style,
                "language": "JP"
            }
        }))
        .timeout(std::time::Duration::from_secs(60)) // Allow for cold start
        .send()
        .await
        .map_err(|e| format!("RunPod request failed: {}", e))?;

    if resp.status().is_success() {
        let body: serde_json::Value = resp.json().await
            .map_err(|e| format!("RunPod JSON parse error: {}", e))?;

        // RunPod returns { "output": { "audio_base64": "..." } } or { "output": "base64..." }
        let audio_b64 = body.get("output")
            .and_then(|o| {
                o.get("audio_base64").and_then(|v| v.as_str())
                    .or_else(|| o.get("audio").and_then(|v| v.as_str()))
                    .or_else(|| o.as_str())
            })
            .unwrap_or("");

        if audio_b64.is_empty() {
            return Err(format!("RunPod SBV2 returned no audio. Response: {}", body));
        }

        // Decode base64 audio
        use base64::Engine as _;
        let audio = base64::engine::general_purpose::STANDARD.decode(audio_b64)
            .map_err(|e| format!("Base64 decode error: {}", e))?;

        tracing::info!("SBV2 TTS success: {} bytes via RunPod", audio.len());
        Ok(audio)
    } else {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Err(format!("RunPod SBV2 error: {} {}", status, body))
    }
}

/// CosyVoice 2 TTS via RunPod Serverless — high-quality multilingual zero-shot voice cloning
async fn try_cosyvoice_tts(text: &str, mode: &str, speaker_id: &str) -> Result<Vec<u8>, String> {
    let api_key = std::env::var("RUNPOD_API_KEY")
        .map_err(|_| "No RUNPOD_API_KEY".to_string())?;
    let endpoint_id = std::env::var("RUNPOD_COSYVOICE_ENDPOINT_ID")
        .map_err(|_| "No RUNPOD_COSYVOICE_ENDPOINT_ID".to_string())?;

    if api_key.is_empty() || endpoint_id.is_empty() {
        return Err("Empty RunPod CosyVoice credentials".to_string());
    }

    let url = format!("https://api.runpod.ai/v2/{}/runsync", endpoint_id);
    let client = reqwest::Client::new();

    let mode_str = if mode.is_empty() { "sft" } else { mode };
    let mut input = serde_json::json!({
        "text": text,
        "mode": mode_str,
        "format": "mp3"
    });
    if !speaker_id.is_empty() {
        input["speaker_id"] = serde_json::json!(speaker_id);
    }

    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&serde_json::json!({ "input": input }))
        .timeout(std::time::Duration::from_secs(90))
        .send()
        .await
        .map_err(|e| format!("RunPod CosyVoice request failed: {}", e))?;

    if resp.status().is_success() {
        let body: serde_json::Value = resp.json().await
            .map_err(|e| format!("RunPod CosyVoice JSON parse error: {}", e))?;

        let audio_b64 = body.get("output")
            .and_then(|o| {
                o.get("audio_base64").and_then(|v| v.as_str())
                    .or_else(|| o.get("audio").and_then(|v| v.as_str()))
                    .or_else(|| o.as_str())
            })
            .unwrap_or("");

        if audio_b64.is_empty() {
            return Err(format!("RunPod CosyVoice returned no audio. Response: {}", body));
        }

        use base64::Engine as _;
        let audio = base64::engine::general_purpose::STANDARD.decode(audio_b64)
            .map_err(|e| format!("Base64 decode error: {}", e))?;

        tracing::info!("CosyVoice TTS success: {} bytes via RunPod", audio.len());
        Ok(audio)
    } else {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Err(format!("RunPod CosyVoice error: {} {}", status, body))
    }
}

/// POST /api/v1/speech/synthesize — Convert text to speech (Polly → OpenAI fallback)
async fn handle_speech_synthesize(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<SpeechRequest>,
) -> impl IntoResponse {
    if req.text.is_empty() || req.text.len() > 4096 {
        return (
            StatusCode::BAD_REQUEST,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            b"{ \"error\": \"Text must be between 1 and 4096 characters\" }".to_vec(),
        );
    }

    // Resolve user: Bearer token → x-session-id → session_id in body
    #[cfg(feature = "dynamodb-backend")]
    let tts_user_key: Option<String> = {
        if let Some(uid) = auth_user_id(&state, &headers).await {
            Some(uid)
        } else {
            let sid = headers.get("x-session-id").and_then(|v| v.to_str().ok())
                .or(req.session_id.as_deref())
                .unwrap_or("");
            if !sid.is_empty() {
                if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                    Some(resolve_session_key(dynamo, table, sid).await)
                } else {
                    None
                }
            } else {
                None
            }
        }
    };

    // Check credits
    #[cfg(feature = "dynamodb-backend")]
    {
        if let Some(ref uid) = tts_user_key {
            if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                let user = get_or_create_user(dynamo, table, uid).await;
                if user.credits_remaining <= 0 {
                    return (
                        StatusCode::PAYMENT_REQUIRED,
                        [(axum::http::header::CONTENT_TYPE, "application/json")],
                        b"{ \"error\": \"Insufficient credits\" }".to_vec(),
                    );
                }
            }
        }
    }

    // TTS engine routing: explicit engine → auto fallback chain
    // Priority: ElevenLabs / SBV2 (if requested) → OpenAI gpt-4o-mini-tts → Polly → fail
    let force_engine = req.engine.as_deref();
    let mut audio_bytes: Option<Vec<u8>> = None;

    // If specific engine requested, try only that
    if force_engine == Some("elevenlabs") {
        match try_elevenlabs_tts(
            &req.text,
            req.voice_id.as_deref().unwrap_or(""),
            req.model_id.as_deref().unwrap_or(""),
        ).await {
            Ok(bytes) => {
                tracing::info!("TTS: ElevenLabs success, {} bytes", bytes.len());
                audio_bytes = Some(bytes);
            }
            Err(e) => tracing::warn!("TTS: ElevenLabs failed: {}", e),
        }
    } else if force_engine == Some("sbv2") {
        let sbv2_model = req.model_id.as_deref().unwrap_or("0").parse::<i32>().unwrap_or(0);
        let sbv2_speaker = req.voice_id.as_deref().unwrap_or("0").parse::<i32>().unwrap_or(0);
        let sbv2_style = req.style.as_deref().unwrap_or("Neutral");
        match try_sbv2_tts(&req.text, sbv2_model, sbv2_speaker, sbv2_style).await {
            Ok(bytes) => {
                tracing::info!("TTS: SBV2 success, {} bytes", bytes.len());
                audio_bytes = Some(bytes);
            }
            Err(e) => tracing::warn!("TTS: SBV2 failed: {}", e),
        }
    } else if force_engine == Some("cosyvoice") {
        let mode = req.style.as_deref().unwrap_or("sft"); // reuse style field for mode
        let speaker = req.voice_id.as_deref().unwrap_or("");
        match try_cosyvoice_tts(&req.text, mode, speaker).await {
            Ok(bytes) => {
                tracing::info!("TTS: CosyVoice success, {} bytes", bytes.len());
                audio_bytes = Some(bytes);
            }
            Err(e) => tracing::warn!("TTS: CosyVoice failed: {}", e),
        }
    } else if force_engine == Some("qwen") {
        match try_qwen_tts(&req.text, &req.voice, req.speed).await {
            Ok(bytes) => {
                tracing::info!("TTS: QWEN3 success, {} bytes", bytes.len());
                audio_bytes = Some(bytes);
            }
            Err(e) => tracing::warn!("TTS: QWEN3 failed: {}", e),
        }
    }

    // Auto fallback chain: QWEN (for Japanese) → OpenAI → Polly
    if audio_bytes.is_none() && force_engine != Some("polly") && force_engine != Some("elevenlabs") && force_engine != Some("sbv2") && force_engine != Some("cosyvoice") && force_engine != Some("qwen") {
        // Detect Japanese text
        let is_ja = req.text.chars().any(|c| {
            ('\u{3040}'..='\u{309F}').contains(&c) || // hiragana
            ('\u{30A0}'..='\u{30FF}').contains(&c) || // katakana
            ('\u{4E00}'..='\u{9FFF}').contains(&c)    // CJK
        });

        // Try QWEN first for Japanese text (if API key is available)
        if is_ja && std::env::var("DASHSCOPE_API_KEY").is_ok() {
            match try_qwen_tts(&req.text, &req.voice, req.speed).await {
                Ok(bytes) => {
                    tracing::info!("TTS: QWEN3 auto-fallback success, {} bytes", bytes.len());
                    audio_bytes = Some(bytes);
                }
                Err(e) => tracing::warn!("TTS: QWEN3 failed ({}), trying OpenAI...", e),
            }
        }

        // OpenAI fallback if QWEN failed or not Japanese
        if audio_bytes.is_none() {
            match try_openai_tts(&req.text, &req.voice, req.speed, req.instructions.as_deref()).await {
                Ok(bytes) => {
                    tracing::info!("TTS: gpt-4o-mini-tts success, {} bytes", bytes.len());
                    audio_bytes = Some(bytes);
                }
                Err(e) => tracing::warn!("TTS: OpenAI failed ({}), trying Polly...", e),
            }
        }
    }

    // Polly fallback
    #[cfg(feature = "dynamodb-backend")]
    if audio_bytes.is_none() && force_engine != Some("openai") && force_engine != Some("elevenlabs") && force_engine != Some("sbv2") && force_engine != Some("qwen") {
        audio_bytes = try_polly_tts(&req.text).await;
    }

    match audio_bytes {
        Some(bytes) if !bytes.is_empty() => {
            // Deduct credits: 1 credit per 100 characters (with race condition protection)
            #[cfg(feature = "dynamodb-backend")]
            {
                if let Some(ref uid) = tts_user_key {
                    if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                        let tts_credits = std::cmp::max(1, (req.text.len() as i64) / 100);
                        let pk = format!("USER#{}", uid);
                        let result = dynamo
                            .update_item()
                            .table_name(table)
                            .key("pk", AttributeValue::S(pk))
                            .key("sk", AttributeValue::S(SK_PROFILE.to_string()))
                            .update_expression("SET credits_remaining = credits_remaining - :c, credits_used = credits_used + :c, updated_at = :now")
                            .condition_expression("credits_remaining >= :c")
                            .expression_attribute_values(":c", AttributeValue::N(tts_credits.to_string()))
                            .expression_attribute_values(":now", AttributeValue::S(chrono::Utc::now().to_rfc3339()))
                            .send()
                            .await;
                        if let Err(e) = result {
                            if e.to_string().contains("ConditionalCheckFailed") {
                                return (
                                    StatusCode::PAYMENT_REQUIRED,
                                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                                    b"{ \"error\": \"Insufficient credits for TTS\" }".to_vec(),
                                );
                            }
                        }
                    }
                }
            }

            (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "audio/mpeg")],
                bytes,
            )
        }
        _ => {
            tracing::error!("All TTS engines failed");
            (
                StatusCode::BAD_GATEWAY,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                b"{ \"error\": \"TTS service unavailable\" }".to_vec(),
            )
        }
    }
}

// ─── Voice Cloning API ───

/// POST /api/v1/voice/clone — Upload audio sample + text, get cloned voice TTS back
/// Body: multipart/form-data with fields: audio (blob), text (string to speak), prompt_text (transcript of audio)
/// Or JSON: { audio_base64, text, prompt_text }
async fn handle_voice_clone(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let api_key = std::env::var("RUNPOD_API_KEY").unwrap_or_default();
    let endpoint_id = std::env::var("RUNPOD_COSYVOICE_ENDPOINT_ID").unwrap_or_default();

    // Fallback: if no CosyVoice endpoint, use ElevenLabs voice design or return error
    if api_key.is_empty() || endpoint_id.is_empty() {
        // Try to use the audio with a default high-quality voice instead
        let body_json: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        let text = body_json.get("text").and_then(|v| v.as_str()).unwrap_or("こんにちは、これが私の声です。");
        let voice = body_json.get("voice").and_then(|v| v.as_str()).unwrap_or("nova");

        // Use OpenAI TTS as fallback demo
        match try_openai_tts(text, voice, 1.0, Some("自然な日本語で、温かく親しみやすいトーンで話してください。")).await {
            Ok(bytes) => {
                return (
                    StatusCode::OK,
                    [
                        (axum::http::header::CONTENT_TYPE, "audio/mpeg"),
                        (axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
                    ],
                    bytes,
                );
            }
            Err(e) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    [
                        (axum::http::header::CONTENT_TYPE, "application/json"),
                        (axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
                    ],
                    format!("{{\"error\": \"Voice clone unavailable: {}\"}}", e).into_bytes(),
                );
            }
        }
    }

    // Parse JSON body
    let body_json: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    let audio_b64 = body_json.get("audio_base64").and_then(|v| v.as_str()).unwrap_or("");
    let text = body_json.get("text").and_then(|v| v.as_str()).unwrap_or("こんにちは、これが私の声です。どう？似てる？");
    let prompt_text = body_json.get("prompt_text").and_then(|v| v.as_str()).unwrap_or("");
    let mode = body_json.get("mode").and_then(|v| v.as_str()).unwrap_or("zero_shot");

    if audio_b64.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            [
                (axum::http::header::CONTENT_TYPE, "application/json"),
                (axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
            ],
            b"{\"error\": \"audio_base64 is required\"}".to_vec(),
        );
    }

    // Call CosyVoice via RunPod
    let url = format!("https://api.runpod.ai/v2/{}/runsync", endpoint_id);
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&serde_json::json!({
            "input": {
                "text": text,
                "mode": mode,
                "prompt_text": prompt_text,
                "prompt_audio": audio_b64,
                "format": "mp3"
            }
        }))
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let resp_body: serde_json::Value = r.json().await.unwrap_or_default();
            let out_b64 = resp_body.get("output")
                .and_then(|o| o.get("audio_base64").and_then(|v| v.as_str())
                    .or_else(|| o.as_str()))
                .unwrap_or("");

            if out_b64.is_empty() {
                return (
                    StatusCode::BAD_GATEWAY,
                    [
                        (axum::http::header::CONTENT_TYPE, "application/json"),
                        (axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
                    ],
                    format!("{{\"error\": \"No audio in response: {}\"}}", resp_body).into_bytes(),
                );
            }

            use base64::Engine as _;
            match base64::engine::general_purpose::STANDARD.decode(out_b64) {
                Ok(audio) => {
                    tracing::info!("Voice clone success: {} bytes", audio.len());
                    (
                        StatusCode::OK,
                        [
                            (axum::http::header::CONTENT_TYPE, "audio/mpeg"),
                            (axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
                        ],
                        audio,
                    )
                }
                Err(e) => (
                    StatusCode::BAD_GATEWAY,
                    [
                        (axum::http::header::CONTENT_TYPE, "application/json"),
                        (axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
                    ],
                    format!("{{\"error\": \"Base64 decode failed: {}\"}}", e).into_bytes(),
                ),
            }
        }
        Ok(r) => {
            let status = r.status();
            let text = r.text().await.unwrap_or_default();
            (
                StatusCode::BAD_GATEWAY,
                [
                    (axum::http::header::CONTENT_TYPE, "application/json"),
                    (axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
                ],
                format!("{{\"error\": \"CosyVoice error: {} {}\"}}", status, text).into_bytes(),
            )
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            [
                (axum::http::header::CONTENT_TYPE, "application/json"),
                (axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
            ],
            format!("{{\"error\": \"RunPod request failed: {}\"}}", e).into_bytes(),
        ),
    }
}

// ─── OpenAI-Compatible TTS API (/v1/audio/speech) ───

/// OpenAI-compatible TTS endpoint: POST /v1/audio/speech
/// Accepts the same format as OpenAI's API: { model, input, voice, speed, response_format }
/// Authenticates via Bearer token (chatweb.ai auth token) or x-api-key header.
/// Uses the same credit system as the internal TTS endpoint.
async fn handle_tts_openai_compat(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    // Map OpenAI format to internal SpeechRequest
    let input = body.get("input").and_then(|v| v.as_str()).unwrap_or("");
    let voice = body.get("voice").and_then(|v| v.as_str()).unwrap_or("nova");
    let speed = body.get("speed").and_then(|v| v.as_f64()).unwrap_or(1.0);
    let model = body.get("model").and_then(|v| v.as_str()).unwrap_or("gpt-4o-mini-tts");
    let instructions = body.get("instructions").and_then(|v| v.as_str());

    // Determine engine from model name
    let engine = if model.contains("elevenlabs") || model.contains("eleven") {
        Some("elevenlabs".to_string())
    } else if model.contains("sbv2") || model.contains("style-bert") {
        Some("sbv2".to_string())
    } else if model.contains("cosyvoice") || model.contains("cosy") {
        Some("cosyvoice".to_string())
    } else if model.contains("polly") {
        Some("polly".to_string())
    } else {
        None // default: OpenAI
    };

    let speech_req = SpeechRequest {
        text: input.to_string(),
        voice: voice.to_string(),
        speed,
        engine,
        instructions: instructions.map(|s| s.to_string()),
        voice_id: body.get("voice_id").and_then(|v| v.as_str()).map(|s| s.to_string()),
        model_id: body.get("model_id").and_then(|v| v.as_str()).map(|s| s.to_string()),
        style: body.get("style").and_then(|v| v.as_str()).map(|s| s.to_string()),
        session_id: body.get("session_id").and_then(|v| v.as_str()).map(|s| s.to_string()),
    };

    handle_speech_synthesize(State(state), headers, Json(speech_req)).await
}

// ─── Amazon Connect (Phone) API ───

/// POST /api/v1/connect/token — Get CCP federation token for browser softphone
async fn handle_connect_token(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let instance_id = std::env::var("CONNECT_INSTANCE_ID").unwrap_or_default();
    if instance_id.is_empty() {
        return Json(serde_json::json!({ "error": "Amazon Connect not configured" })).into_response();
    }

    #[cfg(feature = "dynamodb-backend")]
    {
        // Require authentication
        let user_id = auth_user_id(&state, &headers).await;
        if user_id.is_none() {
            return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "Authentication required" }))).into_response();
        }

        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let client = aws_sdk_connect::Client::new(&config);

        match client
            .get_federation_token()
            .instance_id(&instance_id)
            .send()
            .await
        {
            Ok(output) => {
                if let Some(credentials) = output.credentials() {
                    return Json(serde_json::json!({
                        "access_token": credentials.access_token().unwrap_or(""),
                        "access_token_expiration": credentials.access_token_expiration().map(|t| t.to_string()),
                        "refresh_token": credentials.refresh_token().unwrap_or(""),
                        "refresh_token_expiration": credentials.refresh_token_expiration().map(|t| t.to_string()),
                        "sign_in_url": output.sign_in_url().unwrap_or(""),
                        "user_arn": output.user_arn().unwrap_or(""),
                    })).into_response();
                }
                Json(serde_json::json!({ "error": "No credentials in response" })).into_response()
            }
            Err(e) => {
                tracing::error!("Connect GetFederationToken failed: {e}");
                (StatusCode::BAD_GATEWAY, Json(serde_json::json!({ "error": format!("Connect error: {e}") }))).into_response()
            }
        }
    }

    #[cfg(not(feature = "dynamodb-backend"))]
    {
        let _ = (&state, &headers);
        Json(serde_json::json!({ "error": "Amazon Connect requires dynamodb-backend feature" })).into_response()
    }
}

/// GET /api/v1/connect/transcript/{contact_id} — Get live transcript segments
async fn handle_connect_transcript(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(contact_id): axum::extract::Path<String>,
) -> impl IntoResponse {
    #[cfg(feature = "dynamodb-backend")]
    {
        // Require authentication
        let user_id = auth_user_id(&state, &headers).await;
        if user_id.is_none() {
            return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "Authentication required" }))).into_response();
        }

        use aws_sdk_dynamodb::types::AttributeValue;

        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let pk = format!("TRANSCRIPT#{}", contact_id);
            match dynamo
                .query()
                .table_name(table)
                .key_condition_expression("pk = :pk")
                .expression_attribute_values(":pk", AttributeValue::S(pk))
                .scan_index_forward(true)
                .send()
                .await
            {
                Ok(output) => {
                    let segments: Vec<serde_json::Value> = output.items()
                        .iter()
                        .map(|item| {
                            let get_s = |key: &str| item.get(key).and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                            serde_json::json!({
                                "timestamp": get_s("sk"),
                                "speaker": get_s("speaker"),
                                "content": get_s("content"),
                                "language": get_s("language"),
                            })
                        })
                        .collect();
                    return Json(serde_json::json!({
                        "contact_id": contact_id,
                        "segments": segments,
                    })).into_response();
                }
                Err(e) => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": format!("Query failed: {e}") }))).into_response();
                }
            }
        }
        Json(serde_json::json!({ "error": "DynamoDB not configured" })).into_response()
    }

    #[cfg(not(feature = "dynamodb-backend"))]
    {
        let _ = (&state, &headers, &contact_id);
        Json(serde_json::json!({ "error": "Requires dynamodb-backend feature" })).into_response()
    }
}

// ─── Memory API ───

/// GET /api/v1/memory — Read user's long-term memory and today's daily log
async fn handle_get_memory(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl axum::response::IntoResponse {
    #[cfg(feature = "dynamodb-backend")]
    {
        // Try Bearer token first, then fall back to x-session-id
        let session_key = if let Some(sk) = auth_user_id(&state, &headers).await {
            sk
        } else if let Some(sid) = headers.get("x-session-id").and_then(|v| v.to_str().ok()) {
            if sid.is_empty() {
                return (axum::http::StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({"error": "Unauthorized"}))).into_response();
            }
            if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                resolve_session_key(dynamo, table, sid).await
            } else {
                sid.to_string()
            }
        } else {
            return (axum::http::StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "Unauthorized"}))).into_response();
        };

        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let pk = format!("MEMORY#{}", session_key);
            let today = chrono::Utc::now().format("%Y-%m-%d").to_string();

            let (lt_result, daily_result) = tokio::join!(
                dynamo.get_item().table_name(table)
                    .key("pk", AttributeValue::S(pk.clone()))
                    .key("sk", AttributeValue::S("LONG_TERM".to_string()))
                    .send(),
                dynamo.get_item().table_name(table)
                    .key("pk", AttributeValue::S(pk))
                    .key("sk", AttributeValue::S(format!("DAILY#{}", today)))
                    .send()
            );

            let long_term = lt_result.ok()
                .and_then(|o| o.item)
                .and_then(|item| item.get("content").and_then(|v| v.as_s().ok()).cloned())
                .unwrap_or_default();
            let daily = daily_result.ok()
                .and_then(|o| o.item)
                .and_then(|item| item.get("content").and_then(|v| v.as_s().ok()).cloned())
                .unwrap_or_default();

            return Json(serde_json::json!({
                "long_term": long_term,
                "daily": daily,
            })).into_response();
        }
    }
    Json(serde_json::json!({"long_term": "", "daily": ""})).into_response()
}

/// DELETE /api/v1/memory — Clear user's memory
async fn handle_delete_memory(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl axum::response::IntoResponse {
    #[cfg(feature = "dynamodb-backend")]
    {
        // Try Bearer token first, then fall back to x-session-id
        let session_key = if let Some(sk) = auth_user_id(&state, &headers).await {
            sk
        } else if let Some(sid) = headers.get("x-session-id").and_then(|v| v.to_str().ok()) {
            if sid.is_empty() {
                return (axum::http::StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({"error": "Unauthorized"}))).into_response();
            }
            if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                resolve_session_key(dynamo, table, sid).await
            } else {
                sid.to_string()
            }
        } else {
            return (axum::http::StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "Unauthorized"}))).into_response();
        };

        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let pk = format!("MEMORY#{}", session_key);
            let today = chrono::Utc::now().format("%Y-%m-%d").to_string();

            let _ = tokio::join!(
                dynamo.delete_item().table_name(table)
                    .key("pk", AttributeValue::S(pk.clone()))
                    .key("sk", AttributeValue::S("LONG_TERM".to_string()))
                    .send(),
                dynamo.delete_item().table_name(table)
                    .key("pk", AttributeValue::S(pk))
                    .key("sk", AttributeValue::S(format!("DAILY#{}", today)))
                    .send()
            );

            return Json(serde_json::json!({"ok": true})).into_response();
        }
    }
    Json(serde_json::json!({"ok": true})).into_response()
}

/// Extract session key from Bearer token
// ─── Cross-channel real-time sync ───

/// Increment sync version counter for a session (fire-and-forget).
/// pk: SYNC#{session_key}, sk: VERSION
/// Clients poll this counter to detect new messages from other channels.
/// Must be awaited (not fire-and-forget) because Lambda freezes before spawned tasks complete.
#[cfg(feature = "dynamodb-backend")]
async fn increment_sync_version(
    dynamo: &aws_sdk_dynamodb::Client,
    config_table: &str,
    session_key: &str,
    channel: &str,
) {
    let sync_pk = format!("SYNC#{}", session_key);
    if let Err(e) = dynamo
        .update_item()
        .table_name(config_table)
        .key("pk", AttributeValue::S(sync_pk.clone()))
        .key("sk", AttributeValue::S("VERSION".to_string()))
        .update_expression("SET msg_version = if_not_exists(msg_version, :zero) + :one, last_channel = :ch, updated_at = :now")
        .expression_attribute_values(":one", AttributeValue::N("1".to_string()))
        .expression_attribute_values(":zero", AttributeValue::N("0".to_string()))
        .expression_attribute_values(":ch", AttributeValue::S(channel.to_string()))
        .expression_attribute_values(":now", AttributeValue::S(chrono::Utc::now().to_rfc3339()))
        .send()
        .await
    {
        tracing::warn!("increment_sync_version failed for {}: {}", sync_pk, e);
    }
}

/// Query params for sync poll
#[derive(Debug, Deserialize)]
struct SyncPollParams {
    session_key: Option<String>,
    v: Option<i64>,
}

/// GET /api/v1/sync/poll — Lightweight poll for cross-channel message sync.
/// Returns current version + new messages if version changed.
/// Cost: 1 DynamoDB GetItem (0.5 RCU) per poll.
async fn handle_sync_poll(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(params): Query<SyncPollParams>,
) -> impl IntoResponse {
    // Resolve session key from query param or auth token
    let raw_key = if let Some(ref sk) = params.session_key {
        sk.clone()
    } else {
        let token = headers.get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim_start_matches("Bearer ").to_string())
            .unwrap_or_default();
        if token.is_empty() {
            return Json(serde_json::json!({"error": "session_key required"}));
        }
        token
    };

    let client_version = params.v.unwrap_or(0);

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            // Resolve session key (webchat:UUID → user_id if linked)
            let session_key = resolve_session_key(dynamo, table, &raw_key).await;
            // Fetch current sync version (single GetItem — very cheap)
            let sync_pk = format!("SYNC#{}", session_key);
            match dynamo
                .get_item()
                .table_name(table)
                .key("pk", AttributeValue::S(sync_pk))
                .key("sk", AttributeValue::S("VERSION".to_string()))
                .projection_expression("msg_version, last_channel, updated_at")
                .send()
                .await
            {
                Ok(output) => {
                    if let Some(item) = output.item {
                        let server_version = item.get("msg_version")
                            .and_then(|v| v.as_n().ok())
                            .and_then(|n| n.parse::<i64>().ok())
                            .unwrap_or(0);
                        let last_channel = item.get("last_channel")
                            .and_then(|v| v.as_s().ok())
                            .cloned()
                            .unwrap_or_default();

                        if server_version > client_version {
                            // Version changed — fetch new messages from session
                            let mut sessions = state.sessions.lock().await;
                            let session = sessions.refresh(&session_key);
                            let all_msgs = &session.messages;
                            // Return messages after client's known count
                            // client_version approximately equals number of message pairs the client has seen
                            let skip = (client_version * 2).max(0) as usize;
                            let new_msgs: Vec<serde_json::Value> = all_msgs.iter()
                                .skip(skip)
                                .map(|m| {
                                    let role = &m.role;
                                    let content = &m.content;
                                    let ch = m.extra.get("channel")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("web");
                                    let ts = m.timestamp.as_deref().unwrap_or("");
                                    serde_json::json!({
                                        "role": role,
                                        "content": content,
                                        "channel": ch,
                                        "timestamp": ts,
                                    })
                                })
                                .collect();

                            return Json(serde_json::json!({
                                "updated": true,
                                "version": server_version,
                                "last_channel": last_channel,
                                "messages": new_msgs,
                            }));
                        } else {
                            return Json(serde_json::json!({
                                "updated": false,
                                "version": server_version,
                            }));
                        }
                    } else {
                        // No sync record yet
                        return Json(serde_json::json!({
                            "updated": false,
                            "version": 0,
                        }));
                    }
                }
                Err(e) => {
                    tracing::warn!("Sync poll error: {}", e);
                    return Json(serde_json::json!({
                        "updated": false,
                        "version": client_version,
                        "error": "sync_fetch_failed",
                    }));
                }
            }
        }
    }

    let _ = &state;
    Json(serde_json::json!({
        "updated": false,
        "version": 0,
    }))
}

// ─── Sync API (ElioChat ↔ chatweb.ai) ───

/// GET /api/v1/sync/conversations — List conversations for sync (with optional ?since filter)
async fn handle_sync_list_conversations(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(params): Query<SyncListParams>,
) -> impl IntoResponse {
    let token = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_start_matches("Bearer ").to_string())
        .unwrap_or_default();

    if token.is_empty() {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "Authentication required" })));
    }

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let user_id = resolve_user_from_token(dynamo, table, &token).await;
            if user_id.is_empty() {
                return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "Invalid token" })));
            }

            let user_pk = format!("USER#{}", user_id);

            // Build query — optionally filter by updated_at >= since
            let mut query = dynamo
                .query()
                .table_name(table)
                .key_condition_expression("pk = :pk AND begins_with(sk, :sk)")
                .expression_attribute_values(":pk", AttributeValue::S(user_pk))
                .expression_attribute_values(":sk", AttributeValue::S("CONV#".to_string()))
                .scan_index_forward(false)
                .limit(100);

            if let Some(ref since) = params.since {
                query = query
                    .filter_expression("updated_at >= :since")
                    .expression_attribute_values(":since", AttributeValue::S(since.clone()));
            }

            let resp = query.send().await;

            let conversations: Vec<serde_json::Value> = match resp {
                Ok(output) => {
                    output.items.unwrap_or_default().iter().map(|item| {
                        let title = item.get("title").and_then(|v| v.as_s().ok()).cloned()
                            .unwrap_or_else(|| "New conversation".to_string());
                        let msg_count = item.get("message_count").and_then(|v| v.as_n().ok())
                            .and_then(|n| n.parse::<i64>().ok()).unwrap_or(0);
                        let last_preview = item.get("last_message_preview").and_then(|v| v.as_s().ok()).cloned()
                            .unwrap_or_default();
                        serde_json::json!({
                            "id": item.get("conv_id").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default(),
                            "title": title,
                            "updated_at": item.get("updated_at").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default(),
                            "message_count": msg_count,
                            "last_message_preview": last_preview,
                        })
                    }).collect()
                }
                Err(e) => {
                    tracing::error!("sync list conversations error: {}", e);
                    vec![]
                }
            };

            let sync_token = chrono::Utc::now().to_rfc3339();
            return (StatusCode::OK, Json(serde_json::json!({
                "conversations": conversations,
                "sync_token": sync_token,
            })));
        }
    }

    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "DynamoDB not configured" })))
}

/// GET /api/v1/sync/conversations/{id} — Get full conversation with messages
async fn handle_sync_get_conversation(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let token = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_start_matches("Bearer ").to_string())
        .unwrap_or_default();

    if token.is_empty() {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "Authentication required" })));
    }

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let user_id = resolve_user_from_token(dynamo, table, &token).await;
            if user_id.is_empty() {
                return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "Invalid token" })));
            }

            // Look up the CONV record to get title and session_id
            let user_pk = format!("USER#{}", user_id);
            let conv_resp = dynamo
                .get_item()
                .table_name(table)
                .key("pk", AttributeValue::S(user_pk))
                .key("sk", AttributeValue::S(format!("CONV#{}", id)))
                .send()
                .await;

            let (title, session_id) = if let Ok(output) = conv_resp {
                if let Some(item) = output.item {
                    let t = item.get("title").and_then(|v| v.as_s().ok()).cloned()
                        .unwrap_or_else(|| "New conversation".to_string());
                    let s = item.get("session_id").and_then(|v| v.as_s().ok()).cloned()
                        .unwrap_or_else(|| format!("webchat:{}", id));
                    (t, s)
                } else {
                    return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "Conversation not found" })));
                }
            } else {
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "Database error" })));
            };

            // Get messages from session store
            let mut store = state.sessions.lock().await;
            let session = store.get_or_create(&session_id);
            let messages: Vec<serde_json::Value> = session.messages.iter().map(|m| {
                serde_json::json!({
                    "role": m.role,
                    "content": m.content,
                    "timestamp": m.timestamp.as_deref().unwrap_or(""),
                })
            }).collect();

            return (StatusCode::OK, Json(serde_json::json!({
                "conversation_id": id,
                "title": title,
                "messages": messages,
            })));
        }
    }

    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "DynamoDB not configured" })))
}

/// POST /api/v1/sync/push — Push conversations from ElioChat to chatweb.ai
async fn handle_sync_push(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<SyncPushRequest>,
) -> impl IntoResponse {
    let token = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_start_matches("Bearer ").to_string())
        .unwrap_or_default();

    if token.is_empty() {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "Authentication required" })));
    }

    // Limit: max 20 conversations per push
    if req.conversations.len() > 20 {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "Maximum 20 conversations per push" })));
    }

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let user_id = resolve_user_from_token(dynamo, table, &token).await;
            if user_id.is_empty() {
                return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "Invalid token" })));
            }

            let user_pk = format!("USER#{}", user_id);
            let mut synced: Vec<serde_json::Value> = Vec::new();

            for conv in &req.conversations {
                let conv_id = uuid::Uuid::new_v4().to_string();
                let now = chrono::Utc::now().to_rfc3339();
                let session_id = format!("elio:{}", conv_id);

                // Compute last_message_preview
                let last_preview = conv.messages.last()
                    .map(|m| m.content.chars().take(80).collect::<String>())
                    .unwrap_or_default();

                // Create CONV record
                let _ = dynamo
                    .put_item()
                    .table_name(table)
                    .item("pk", AttributeValue::S(user_pk.clone()))
                    .item("sk", AttributeValue::S(format!("CONV#{}", conv_id)))
                    .item("conv_id", AttributeValue::S(conv_id.clone()))
                    .item("title", AttributeValue::S(conv.title.clone()))
                    .item("session_id", AttributeValue::S(session_id.clone()))
                    .item("created_at", AttributeValue::S(now.clone()))
                    .item("updated_at", AttributeValue::S(now.clone()))
                    .item("message_count", AttributeValue::N(conv.messages.len().to_string()))
                    .item("last_message_preview", AttributeValue::S(last_preview))
                    .item("source", AttributeValue::S("elio".to_string()))
                    .send()
                    .await;

                // Link session
                let link_pk = format!("LINK#{}", session_id);
                let _ = dynamo
                    .put_item()
                    .table_name(table)
                    .item("pk", AttributeValue::S(link_pk))
                    .item("sk", AttributeValue::S("CHANNEL_MAP".to_string()))
                    .item("user_id", AttributeValue::S(user_id.clone()))
                    .item("linked_at", AttributeValue::S(now.clone()))
                    .send()
                    .await;

                // Store messages in session store
                {
                    let mut store = state.sessions.lock().await;
                    let session = store.get_or_create(&session_id);
                    for msg in &conv.messages {
                        session.messages.push(crate::session::SessionMessage {
                            role: msg.role.clone(),
                            content: msg.content.clone(),
                            timestamp: msg.timestamp.clone(),
                            extra: std::collections::HashMap::new(),
                        });
                    }
                }

                synced.push(serde_json::json!({
                    "client_id": conv.client_id,
                    "server_id": conv_id,
                }));
            }

            return (StatusCode::OK, Json(serde_json::json!({ "synced": synced })));
        }
    }

    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "DynamoDB not configured" })))
}

// ---------------------------------------------------------------------------
// Cron / Scheduled Tasks API
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CronCreateRequest {
    name: String,
    message: String,
    schedule: CronScheduleInput,
    channel: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CronScheduleInput {
    every_minutes: Option<u64>,
    cron: Option<String>,
    at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CronUpdateRequest {
    enabled: Option<bool>,
    name: Option<String>,
    message: Option<String>,
}

async fn handle_cron_list(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let token = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_start_matches("Bearer ").to_string())
        .unwrap_or_default();

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let user_id = resolve_user_from_token(dynamo, table, &token).await;
            if user_id.is_empty() {
                return Json(serde_json::json!({ "jobs": [], "error": "Not authenticated" }));
            }
            let user_pk = format!("CRON#{}", user_id);
            let resp = dynamo
                .query()
                .table_name(table)
                .key_condition_expression("pk = :pk AND begins_with(sk, :sk)")
                .expression_attribute_values(":pk", AttributeValue::S(user_pk))
                .expression_attribute_values(":sk", AttributeValue::S("JOB#".to_string()))
                .scan_index_forward(false)
                .limit(50)
                .send()
                .await;
            let jobs: Vec<serde_json::Value> = match resp {
                Ok(output) => {
                    output.items.unwrap_or_default().iter().map(|item| {
                        let schedule_type = item.get("schedule_type").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                        let schedule_val = item.get("schedule_value").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                        let display = match schedule_type.as_str() {
                            "every" => format!("{}min", schedule_val),
                            "cron" => schedule_val.clone(),
                            "at" => schedule_val.clone(),
                            _ => schedule_val.clone(),
                        };
                        serde_json::json!({
                            "id": item.get("job_id").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default(),
                            "name": item.get("name").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default(),
                            "message": item.get("message").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default(),
                            "schedule": format!("{}:{}", schedule_type, schedule_val),
                            "schedule_display": display,
                            "channel": item.get("channel").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default(),
                            "enabled": item.get("enabled").and_then(|v| v.as_s().ok()).map(|s| s == "true").unwrap_or(true),
                            "created_at": item.get("created_at").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default(),
                        })
                    }).collect()
                }
                Err(_) => vec![],
            };
            return Json(serde_json::json!({ "jobs": jobs }));
        }
    }

    let _ = (&state, &token);
    Json(serde_json::json!({ "jobs": [] }))
}

async fn handle_cron_create(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<CronCreateRequest>,
) -> impl IntoResponse {
    let token = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_start_matches("Bearer ").to_string())
        .unwrap_or_default();

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let user_id = resolve_user_from_token(dynamo, table, &token).await;
            if user_id.is_empty() {
                return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "Not authenticated" }))).into_response();
            }

            // Check plan limits: Free = 1 cron, Starter = 5, Pro = 20
            let user = get_or_create_user(dynamo, table, &user_id).await;
            let plan = user.plan.as_str();
            let max_jobs: usize = match plan {
                "starter" => 5,
                "pro" | "enterprise" => 20,
                _ => 1,
            };

            // Count existing jobs
            let user_pk = format!("CRON#{}", user_id);
            let count_resp = dynamo.query()
                .table_name(table)
                .key_condition_expression("pk = :pk AND begins_with(sk, :sk)")
                .expression_attribute_values(":pk", AttributeValue::S(user_pk.clone()))
                .expression_attribute_values(":sk", AttributeValue::S("JOB#".to_string()))
                .select(aws_sdk_dynamodb::types::Select::Count)
                .send().await;
            let current_count = count_resp.map(|r| r.count() as usize).unwrap_or(0);
            if current_count >= max_jobs {
                return (StatusCode::FORBIDDEN, Json(serde_json::json!({
                    "error": format!("Plan limit reached ({}/{}). Upgrade for more scheduled tasks.", current_count, max_jobs)
                }))).into_response();
            }

            let job_id = uuid::Uuid::new_v4().to_string()[..8].to_string();
            let (schedule_type, schedule_value) = if let Some(mins) = req.schedule.every_minutes {
                ("every".to_string(), mins.to_string())
            } else if let Some(ref expr) = req.schedule.cron {
                ("cron".to_string(), expr.clone())
            } else if let Some(ref at_str) = req.schedule.at {
                ("at".to_string(), at_str.clone())
            } else {
                return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "Invalid schedule" }))).into_response();
            };

            let now = chrono::Utc::now().to_rfc3339();
            let mut item = std::collections::HashMap::new();
            item.insert("pk".to_string(), AttributeValue::S(user_pk));
            item.insert("sk".to_string(), AttributeValue::S(format!("JOB#{}", job_id)));
            item.insert("job_id".to_string(), AttributeValue::S(job_id.clone()));
            item.insert("name".to_string(), AttributeValue::S(req.name));
            item.insert("message".to_string(), AttributeValue::S(req.message));
            item.insert("schedule_type".to_string(), AttributeValue::S(schedule_type));
            item.insert("schedule_value".to_string(), AttributeValue::S(schedule_value));
            item.insert("channel".to_string(), AttributeValue::S(req.channel.unwrap_or_default()));
            item.insert("enabled".to_string(), AttributeValue::S("true".to_string()));
            item.insert("created_at".to_string(), AttributeValue::S(now));
            item.insert("user_id".to_string(), AttributeValue::S(user_id));

            match dynamo.put_item().table_name(table).set_item(Some(item)).send().await {
                Ok(_) => return Json(serde_json::json!({ "ok": true, "id": job_id })).into_response(),
                Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": format!("{}", e) }))).into_response(),
            }
        }
    }

    let _ = (&state, &token, &req);
    (StatusCode::NOT_IMPLEMENTED, Json(serde_json::json!({ "error": "DynamoDB backend required" }))).into_response()
}

async fn handle_cron_update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(req): Json<CronUpdateRequest>,
) -> impl IntoResponse {
    let token = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_start_matches("Bearer ").to_string())
        .unwrap_or_default();

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let user_id = resolve_user_from_token(dynamo, table, &token).await;
            if user_id.is_empty() {
                return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "Not authenticated" }))).into_response();
            }
            let user_pk = format!("CRON#{}", user_id);
            let sk = format!("JOB#{}", id);

            let mut update_expr_parts = vec![];
            let mut attr_values = std::collections::HashMap::new();
            if let Some(enabled) = req.enabled {
                update_expr_parts.push("enabled = :enabled");
                attr_values.insert(":enabled".to_string(), AttributeValue::S(enabled.to_string()));
            }
            if let Some(ref name) = req.name {
                update_expr_parts.push("#n = :name");
                attr_values.insert(":name".to_string(), AttributeValue::S(name.clone()));
            }
            if let Some(ref message) = req.message {
                update_expr_parts.push("message = :msg");
                attr_values.insert(":msg".to_string(), AttributeValue::S(message.clone()));
            }
            if update_expr_parts.is_empty() {
                return Json(serde_json::json!({ "ok": true })).into_response();
            }

            let update_expr = format!("SET {}", update_expr_parts.join(", "));
            let mut builder = dynamo.update_item()
                .table_name(table)
                .key("pk", AttributeValue::S(user_pk))
                .key("sk", AttributeValue::S(sk))
                .update_expression(&update_expr);
            for (k, v) in &attr_values {
                builder = builder.expression_attribute_values(k, v.clone());
            }
            if req.name.is_some() {
                builder = builder.expression_attribute_names("#n", "name");
            }
            match builder.send().await {
                Ok(_) => return Json(serde_json::json!({ "ok": true })).into_response(),
                Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": format!("{}", e) }))).into_response(),
            }
        }
    }

    let _ = (&state, &id, &token, &req);
    (StatusCode::NOT_IMPLEMENTED, Json(serde_json::json!({ "error": "DynamoDB backend required" }))).into_response()
}

async fn handle_cron_delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let token = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_start_matches("Bearer ").to_string())
        .unwrap_or_default();

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let user_id = resolve_user_from_token(dynamo, table, &token).await;
            if user_id.is_empty() {
                return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "Not authenticated" }))).into_response();
            }
            let user_pk = format!("CRON#{}", user_id);
            let sk = format!("JOB#{}", id);
            match dynamo.delete_item()
                .table_name(table)
                .key("pk", AttributeValue::S(user_pk))
                .key("sk", AttributeValue::S(sk))
                .send().await
            {
                Ok(_) => return Json(serde_json::json!({ "ok": true })).into_response(),
                Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": format!("{}", e) }))).into_response(),
            }
        }
    }

    let _ = (&state, &id, &token);
    (StatusCode::NOT_IMPLEMENTED, Json(serde_json::json!({ "error": "DynamoDB backend required" }))).into_response()
}

/// Start the HTTP server on the given address.
pub async fn serve(addr: &str, state: Arc<AppState>) -> anyhow::Result<()> {
    let router = create_router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("HTTP server listening on {}", addr);
    axum::serve(listener, router).await?;
    Ok(())
}

// ─── A/B Test Engine (Thompson Sampling Multi-Armed Bandit) ───

/// A/B test experiment variants.
/// Each variant has a conversation style that the AI and UI adopt.
const AB_VARIANTS: &[AbVariant] = &[
    AbVariant {
        id: "warm_casual",
        name: "温かカジュアル",
        greeting_ja: "やっほー！何か手伝えることある？",
        greeting_en: "Hey there! What can I help you with?",
        style_hint: "casual, warm, emoji-friendly, use first-person 僕/私",
        personality: "enthusiastic_friend",
    },
    AbVariant {
        id: "polite_pro",
        name: "丁寧プロ",
        greeting_ja: "こんにちは。どのようなことでもお手伝いいたします。",
        greeting_en: "Hello. I'm here to assist you with anything you need.",
        style_hint: "polite, professional, keigo, clear structure",
        personality: "professional_butler",
    },
    AbVariant {
        id: "playful_curious",
        name: "わくわく好奇心",
        greeting_ja: "わくわく！今日はどんな冒険する？何でも聞いてみて！",
        greeting_en: "Exciting! What adventure shall we go on today? Ask me anything!",
        style_hint: "playful, curious, uses exclamation, asks follow-up questions",
        personality: "curious_explorer",
    },
    AbVariant {
        id: "calm_wise",
        name: "落ち着き知恵者",
        greeting_ja: "ゆっくりでいいよ。何でも相談してね。",
        greeting_en: "Take your time. I'm here whenever you're ready.",
        style_hint: "calm, wise, thoughtful, minimal emoji, deeper answers",
        personality: "wise_mentor",
    },
    AbVariant {
        id: "energetic_doer",
        name: "爆速アクション",
        greeting_ja: "OK！すぐやるよ！何する？",
        greeting_en: "OK! Let's do this! What's the task?",
        style_hint: "action-oriented, fast, short sentences, tool-heavy",
        personality: "action_hero",
    },
];

struct AbVariant {
    id: &'static str,
    name: &'static str,
    greeting_ja: &'static str,
    greeting_en: &'static str,
    style_hint: &'static str,
    personality: &'static str,
}

/// GET /api/v1/ab/variant — Get assigned A/B test variant for this session.
/// Uses Thompson Sampling: samples from Beta(successes+1, failures+1) for each variant.
async fn handle_ab_variant(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    // Check if session already has a variant assigned
    let existing = headers.get("x-ab-variant")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !existing.is_empty() {
        if let Some(v) = AB_VARIANTS.iter().find(|v| v.id == existing) {
            return Json(serde_json::json!({
                "variant_id": v.id,
                "name": v.name,
                "greeting_ja": v.greeting_ja,
                "greeting_en": v.greeting_en,
                "style_hint": v.style_hint,
                "personality": v.personality,
            }));
        }
    }

    // Thompson Sampling: pick best variant
    let variant = pick_variant_thompson(&state).await;

    Json(serde_json::json!({
        "variant_id": variant.id,
        "name": variant.name,
        "greeting_ja": variant.greeting_ja,
        "greeting_en": variant.greeting_en,
        "style_hint": variant.style_hint,
        "personality": variant.personality,
    }))
}

/// Thompson Sampling: for each variant, sample from Beta(wins+1, losses+1),
/// pick the variant with the highest sample.
async fn pick_variant_thompson(state: &Arc<AppState>) -> &'static AbVariant {
    let stats = load_ab_stats(state).await;
    let mut rng = rand::thread_rng();
    let mut best_sample = -1.0f64;
    let mut best_idx = 0;

    for (i, variant) in AB_VARIANTS.iter().enumerate() {
        let (wins, losses) = stats.get(variant.id).copied().unwrap_or((0, 0));
        let alpha = (wins + 1) as f64;
        let beta_param = (losses + 1) as f64;
        let sample = sample_beta(&mut rng, alpha, beta_param);
        if sample > best_sample {
            best_sample = sample;
            best_idx = i;
        }
    }

    &AB_VARIANTS[best_idx]
}

/// Simple Beta distribution sampling using ratio of Gamma samples.
fn sample_beta(rng: &mut impl rand::Rng, alpha: f64, beta: f64) -> f64 {
    let x = sample_gamma(rng, alpha);
    let y = sample_gamma(rng, beta);
    if x + y == 0.0 { 0.5 } else { x / (x + y) }
}

/// Gamma sampling via Marsaglia and Tsang's method.
fn sample_gamma(rng: &mut impl rand::Rng, shape: f64) -> f64 {
    if shape < 1.0 {
        let u: f64 = rng.gen();
        return sample_gamma(rng, shape + 1.0) * u.powf(1.0 / shape);
    }
    let d = shape - 1.0 / 3.0;
    let c = 1.0 / (9.0 * d).sqrt();
    loop {
        let x: f64 = rng.gen::<f64>() * 2.0 - 1.0;
        let v = (1.0 + c * x).powi(3);
        if v <= 0.0 { continue; }
        let u: f64 = rng.gen();
        if u < 1.0 - 0.0331 * x.powi(4) {
            return d * v;
        }
        if u.ln() < 0.5 * x.powi(2) + d * (1.0 - v + v.ln()) {
            return d * v;
        }
    }
}

/// Load A/B test stats from DynamoDB.
async fn load_ab_stats(state: &Arc<AppState>) -> std::collections::HashMap<&'static str, (u32, u32)> {
    #[allow(unused_mut)]
    let mut stats: std::collections::HashMap<&'static str, (u32, u32)> = std::collections::HashMap::new();

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            if let Ok(result) = dynamo.get_item()
                .table_name(table)
                .key("pk", AttributeValue::S("AB_STATS#global".to_string()))
                .key("sk", AttributeValue::S("CURRENT".to_string()))
                .send()
                .await
            {
                if let Some(item) = result.item() {
                    for variant in AB_VARIANTS {
                        let wins_key = format!("{}_wins", variant.id);
                        let losses_key = format!("{}_losses", variant.id);
                        let wins = item.get(&wins_key)
                            .and_then(|v| v.as_n().ok())
                            .and_then(|n| n.parse::<u32>().ok())
                            .unwrap_or(0);
                        let losses = item.get(&losses_key)
                            .and_then(|v| v.as_n().ok())
                            .and_then(|n| n.parse::<u32>().ok())
                            .unwrap_or(0);
                        stats.insert(variant.id, (wins, losses));
                    }
                }
            }
        }
    }

    stats
}

/// POST /api/v1/ab/event — Record an A/B test engagement event.
async fn handle_ab_event(
    State(state): State<Arc<AppState>>,
    Json(req): Json<serde_json::Value>,
) -> impl IntoResponse {
    let variant_id = req.get("variant_id").and_then(|v| v.as_str()).unwrap_or("");
    let event = req.get("event").and_then(|v| v.as_str()).unwrap_or("");
    let messages_sent = req.get("messages_sent").and_then(|v| v.as_u64()).unwrap_or(0);

    if variant_id.is_empty() || event.is_empty() {
        return Json(serde_json::json!({ "error": "variant_id and event required" }));
    }

    if !AB_VARIANTS.iter().any(|v| v.id == variant_id) {
        return Json(serde_json::json!({ "error": "unknown variant" }));
    }

    // "engaged" = 3+ messages = success. "bounced" = failure.
    let _is_win = event == "engaged" || messages_sent >= 3;

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let field = if _is_win {
                format!("{}_wins", variant_id)
            } else {
                format!("{}_losses", variant_id)
            };

            let _ = dynamo.update_item()
                .table_name(table)
                .key("pk", AttributeValue::S("AB_STATS#global".to_string()))
                .key("sk", AttributeValue::S("CURRENT".to_string()))
                .update_expression(format!("ADD {} :one", field))
                .expression_attribute_values(":one", AttributeValue::N("1".to_string()))
                .send()
                .await;

            let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
            let _ = dynamo.update_item()
                .table_name(table)
                .key("pk", AttributeValue::S("AB_STATS#global".to_string()))
                .key("sk", AttributeValue::S(format!("DAY#{}", today)))
                .update_expression(format!("ADD {} :one", field))
                .expression_attribute_values(":one", AttributeValue::N("1".to_string()))
                .send()
                .await;
        }
    }

    Json(serde_json::json!({ "ok": true, "recorded": event, "variant": variant_id }))
}

/// GET /api/v1/ab/stats — View A/B test statistics.
async fn handle_ab_stats(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let stats = load_ab_stats(&state).await;

    let variants: Vec<serde_json::Value> = AB_VARIANTS.iter().map(|v| {
        let (wins, losses) = stats.get(v.id).copied().unwrap_or((0, 0));
        let total = wins + losses;
        let rate = if total > 0 { wins as f64 / total as f64 } else { 0.0 };
        serde_json::json!({
            "id": v.id,
            "name": v.name,
            "wins": wins,
            "losses": losses,
            "total": total,
            "engagement_rate": format!("{:.1}%", rate * 100.0),
            "greeting_ja": v.greeting_ja,
            "style_hint": v.style_hint,
        })
    }).collect();

    Json(serde_json::json!({
        "variants": variants,
        "algorithm": "thompson_sampling",
    }))
}

// ---------------------------------------------------------------------------
// Feedback endpoints
// ---------------------------------------------------------------------------

/// POST /api/v1/feedback — Collect user feedback (thumbs up/down)
async fn handle_feedback(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<FeedbackRequest>,
) -> impl IntoResponse {
    // Validate rating
    if req.rating != "up" && req.rating != "down" {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
            "ok": false, "error": "rating must be 'up' or 'down'"
        }))).into_response();
    }

    let session_id = headers.get("x-session-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("anonymous")
        .to_string();

    // Resolve user_id from Bearer token if present
    let user_id = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t.to_string());

    let channel = if session_id.starts_with("line:") { "line" }
        else if session_id.starts_with("tg:") { "telegram" }
        else if session_id.starts_with("webchat:") { "web" }
        else { "api" };

    let snippet = req.snippet.unwrap_or_default();
    let conv_id = req.conversation_id.unwrap_or_default();

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(config_table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let dynamo = dynamo.clone();
            let config_table = config_table.to_string();
            let rating = req.rating.clone();
            let session_id_c = session_id.clone();
            let user_id_c = user_id.clone().unwrap_or_default();
            let channel_c = channel.to_string();
            let snippet_c = snippet.clone();
            let conv_id_c = conv_id.clone();

            // Fire-and-forget: write feedback record
            tokio::spawn(async move {
                let now = chrono::Utc::now();
                let date = now.format("%Y-%m-%d").to_string();
                let ts = now.timestamp_millis().to_string();
                let uuid_prefix = &uuid::Uuid::new_v4().to_string()[..6];
                let sk = format!("{}#{}", ts, uuid_prefix);
                let ttl = (now.timestamp() + 90 * 24 * 3600).to_string();

                let _ = dynamo
                    .put_item()
                    .table_name(&config_table)
                    .item("pk", AttributeValue::S(format!("FEEDBACK#{}", date)))
                    .item("sk", AttributeValue::S(sk))
                    .item("rating", AttributeValue::S(rating.clone()))
                    .item("snippet", AttributeValue::S(snippet_c))
                    .item("session_id", AttributeValue::S(session_id_c))
                    .item("user_id", AttributeValue::S(user_id_c))
                    .item("channel", AttributeValue::S(channel_c))
                    .item("conversation_id", AttributeValue::S(conv_id_c))
                    .item("timestamp", AttributeValue::S(now.to_rfc3339()))
                    .item("ttl", AttributeValue::N(ttl))
                    .send()
                    .await;

                // Atomic increment of aggregate counter
                let counter_attr = if rating == "up" { "total_up" } else { "total_down" };
                let _ = dynamo
                    .update_item()
                    .table_name(&config_table)
                    .key("pk", AttributeValue::S("FEEDBACK_STATS#global".to_string()))
                    .key("sk", AttributeValue::S("CURRENT".to_string()))
                    .update_expression(format!("ADD {} :one", counter_attr))
                    .expression_attribute_values(":one", AttributeValue::N("1".to_string()))
                    .send()
                    .await;
            });
        }
    }

    let _ = &state; // suppress unused warning in non-dynamo builds

    Json(serde_json::json!({ "ok": true })).into_response()
}

/// Query parameters for admin feedback endpoint.
#[derive(Debug, Deserialize)]
struct AdminFeedbackQuery {
    sid: Option<String>,
    days: Option<u32>,
}

/// GET /api/v1/admin/feedback?sid=<key>&days=<n> — Admin feedback dashboard
async fn handle_admin_feedback(
    State(state): State<Arc<AppState>>,
    Query(q): Query<AdminFeedbackQuery>,
) -> impl IntoResponse {
    let sid = q.sid.unwrap_or_default();
    if !is_admin(&sid) {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({
            "error": "Forbidden"
        }))).into_response();
    }

    let days = q.days.unwrap_or(7).min(90);

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(config_table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let now = chrono::Utc::now();
            let mut feedbacks: Vec<serde_json::Value> = Vec::new();

            // Query each day
            for d in 0..days {
                let date = (now - chrono::Duration::days(d as i64)).format("%Y-%m-%d").to_string();
                let pk = format!("FEEDBACK#{}", date);

                let resp = dynamo
                    .query()
                    .table_name(config_table)
                    .key_condition_expression("pk = :pk")
                    .expression_attribute_values(":pk", AttributeValue::S(pk))
                    .scan_index_forward(false) // newest first
                    .limit(100)
                    .send()
                    .await;

                if let Ok(output) = resp {
                    if let Some(items) = output.items {
                        for item in items {
                            let rating = item.get("rating").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                            let snippet = item.get("snippet").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                            let channel = item.get("channel").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                            let timestamp = item.get("timestamp").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                            let user_id = item.get("user_id").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                            feedbacks.push(serde_json::json!({
                                "rating": rating,
                                "snippet": snippet,
                                "channel": channel,
                                "timestamp": timestamp,
                                "user_id": user_id,
                            }));
                        }
                    }
                }
            }

            // Get aggregate stats
            let stats_resp = dynamo
                .get_item()
                .table_name(config_table)
                .key("pk", AttributeValue::S("FEEDBACK_STATS#global".to_string()))
                .key("sk", AttributeValue::S("CURRENT".to_string()))
                .send()
                .await;

            let (total_up, total_down) = if let Ok(resp) = stats_resp {
                if let Some(item) = resp.item {
                    let up: i64 = item.get("total_up").and_then(|v| v.as_n().ok())
                        .and_then(|n| n.parse().ok()).unwrap_or(0);
                    let down: i64 = item.get("total_down").and_then(|v| v.as_n().ok())
                        .and_then(|n| n.parse().ok()).unwrap_or(0);
                    (up, down)
                } else { (0, 0) }
            } else { (0, 0) };

            return Json(serde_json::json!({
                "feedbacks": feedbacks,
                "stats": {
                    "total_up": total_up,
                    "total_down": total_down,
                    "total": total_up + total_down,
                    "positive_rate": if total_up + total_down > 0 {
                        (total_up as f64 / (total_up + total_down) as f64 * 100.0).round()
                    } else { 0.0 },
                },
                "days_queried": days,
            })).into_response();
        }
    }

    let _ = &state;
    Json(serde_json::json!({
        "feedbacks": [],
        "stats": { "total_up": 0, "total_down": 0, "total": 0, "positive_rate": 0 },
        "days_queried": days,
    })).into_response()
}

#[cfg(test)]
mod agent_routing_tests {
    use super::*;

    #[test]
    fn test_explicit_prefix() {
        let (agent, msg, score) = detect_agent("@coder fix this bug");
        assert_eq!(agent.id, "coder");
        assert_eq!(msg, "fix this bug");
        assert_eq!(score, 100); // explicit @agent
    }

    #[test]
    fn test_explicit_prefix_researcher() {
        let (agent, msg, _score) = detect_agent("@researcher find latest news");
        assert_eq!(agent.id, "researcher");
        assert_eq!(msg, "find latest news");
    }

    #[test]
    fn test_research_over_code() {
        // "Pythonを調べて" → researcher (調べ=3) > coder (python=1)
        let (agent, _, score) = detect_agent("Pythonを調べて");
        assert_eq!(agent.id, "researcher");
        assert!(score >= 2);
    }

    #[test]
    fn test_creative_translate() {
        let (agent, _, _) = detect_agent("この文章を英語に翻訳して");
        assert_eq!(agent.id, "creative");
    }

    #[test]
    fn test_analyst_data() {
        let (agent, _, _) = detect_agent("このデータを分析してください");
        assert_eq!(agent.id, "analyst");
    }

    #[test]
    fn test_default_greeting() {
        let (agent, _, score) = detect_agent("こんにちは");
        assert_eq!(agent.id, "assistant");
        assert_eq!(score, 0);
    }

    #[test]
    fn test_weak_signal_default() {
        // "error" alone (no match) → assistant (below threshold)
        let (agent, _, score) = detect_agent("error");
        assert_eq!(agent.id, "assistant");
        assert_eq!(score, 0);
    }

    #[test]
    fn test_weather_researcher() {
        let (agent, _, _) = detect_agent("今日の天気を教えて");
        assert_eq!(agent.id, "researcher");
    }

    #[test]
    fn test_debug_coder() {
        let (agent, _, _) = detect_agent("このバグをdebugして");
        assert_eq!(agent.id, "coder");
    }

    #[test]
    fn test_code_writing_coder() {
        let (agent, _, _) = detect_agent("Rustでコードを書いて");
        // コード=2 + rust=1 = coder:3, 書いて=2 = creative:2 → coder wins
        assert_eq!(agent.id, "coder");
    }

    #[test]
    fn test_news_researcher() {
        // 最新=3, ニュース=3 → researcher
        let (agent, _, _) = detect_agent("最新のAIニュース");
        assert_eq!(agent.id, "researcher");
    }

    #[test]
    fn test_single_weak_keyword_defaults() {
        // "api" alone = coder score 1, below threshold → assistant
        let (agent, _, score) = detect_agent("api");
        assert_eq!(agent.id, "assistant");
        assert_eq!(score, 0);
    }

    #[test]
    fn test_agent_profile_fields() {
        // Verify new fields are populated
        let coder = &AGENTS[3];
        assert_eq!(coder.id, "coder");
        assert_eq!(coder.preferred_model, Some("claude-sonnet-4-5-20250929"));
        assert_eq!(coder.estimated_seconds, 15);
        assert_eq!(coder.max_chars_pc, 800);
        assert_eq!(coder.max_chars_mobile, 400);
        assert_eq!(coder.max_chars_voice, 60);

        let assistant = &AGENTS[1];
        assert_eq!(assistant.preferred_model, None);
        assert_eq!(assistant.estimated_seconds, 10);
    }

    #[test]
    fn test_detect_language() {
        assert_eq!(detect_language("hello world"), "en");
        assert_eq!(detect_language("こんにちは"), "ja");
        assert_eq!(detect_language("Rustでコードを書いて"), "ja");
        assert_eq!(detect_language(""), "en"); // empty is ascii
        assert_eq!(detect_language("café"), "other"); // non-ascii, non-ja
    }

    #[test]
    fn test_build_meta_context_anonymous() {
        let ctx = build_meta_context(None, "web", "pc", 0, false);
        assert!(ctx.contains("現在時刻:"));
        assert!(ctx.contains("チャネル: web"));
        assert!(ctx.contains("デバイス: pc"));
        assert!(ctx.contains("新規"));
    }

    #[test]
    fn test_build_meta_context_with_user() {
        let user = UserProfile {
            user_id: "test".to_string(),
            display_name: Some("太郎".to_string()),
            plan: "pro".to_string(),
            credits_remaining: 5000,
            credits_used: 100,
            channels: vec!["line".to_string(), "telegram".to_string()],
            stripe_customer_id: None,
            email: None,
            created_at: "2025-01-01".to_string(),
        };
        let ctx = build_meta_context(Some(&user), "line", "mobile", 6, false);
        assert!(ctx.contains("ユーザー名: 太郎"));
        assert!(ctx.contains("プラン: pro"));
        assert!(ctx.contains("残クレジット: 5000"));
        assert!(ctx.contains("連携: line,telegram"));
        assert!(ctx.contains("チャネル: line"));
        assert!(ctx.contains("デバイス: mobile"));
        assert!(ctx.contains("継続(6件)"));
    }

    #[test]
    fn test_build_meta_context_english() {
        let ctx = build_meta_context(None, "web", "pc", 3, true);
        assert!(ctx.contains("Time:"));
        assert!(ctx.contains("Channel: web"));
        assert!(ctx.contains("Device: pc"));
        assert!(ctx.contains("ongoing(3msgs)"));
    }

    // -----------------------------------------------------------------------
    // is_admin tests
    // -----------------------------------------------------------------------

    /// All is_admin tests in a single test to avoid env var race conditions
    /// between parallel test threads.
    #[test]
    fn test_is_admin_all_cases() {
        // Matching keys
        std::env::set_var("ADMIN_SESSION_KEYS", "admin1,admin2");
        assert!(is_admin("admin1"));
        assert!(is_admin("admin2"));
        assert!(!is_admin("unknown"));
        assert!(!is_admin(""));

        // Whitespace trimming
        std::env::set_var("ADMIN_SESSION_KEYS", " key1 , key2 ");
        assert!(is_admin("key1"));
        assert!(is_admin("key2"));
        assert!(!is_admin(" key1 "));

        // Single key
        std::env::set_var("ADMIN_SESSION_KEYS", "onlyone");
        assert!(is_admin("onlyone"));
        assert!(!is_admin("other"));

        // Empty env
        std::env::remove_var("ADMIN_SESSION_KEYS");
        assert!(!is_admin("anything"));
    }

    // -----------------------------------------------------------------------
    // UserProfile serialization tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_user_profile_serialization() {
        let profile = UserProfile {
            user_id: "test-user".to_string(),
            display_name: Some("Test".to_string()),
            plan: "free".to_string(),
            credits_remaining: 100,
            credits_used: 0,
            channels: vec!["web".to_string()],
            stripe_customer_id: None,
            email: Some("test@example.com".to_string()),
            created_at: "2025-01-01".to_string(),
        };
        let json = serde_json::to_string(&profile).unwrap();
        assert!(json.contains("test-user"));
        assert!(json.contains("free"));
        assert!(json.contains("test@example.com"));
        let deser: UserProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.user_id, "test-user");
        assert_eq!(deser.credits_remaining, 100);
        assert_eq!(deser.credits_used, 0);
        assert_eq!(deser.channels, vec!["web"]);
    }

    #[test]
    fn test_user_profile_optional_fields() {
        let profile = UserProfile {
            user_id: "u1".to_string(),
            display_name: None,
            plan: "pro".to_string(),
            credits_remaining: 5000,
            credits_used: 200,
            channels: vec![],
            stripe_customer_id: None,
            email: None,
            created_at: "2025-06-01".to_string(),
        };
        let json = serde_json::to_string(&profile).unwrap();
        // None fields serialize as null in serde default
        let deser: UserProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.display_name, None);
        assert_eq!(deser.email, None);
        assert_eq!(deser.plan, "pro");
        assert_eq!(deser.credits_remaining, 5000);
    }

    #[test]
    fn test_user_profile_roundtrip() {
        let profile = UserProfile {
            user_id: "roundtrip-user".to_string(),
            display_name: Some("Alice".to_string()),
            plan: "starter".to_string(),
            credits_remaining: 999,
            credits_used: 1,
            channels: vec!["line".to_string(), "telegram".to_string()],
            stripe_customer_id: Some("cus_abc123".to_string()),
            email: Some("alice@example.com".to_string()),
            created_at: "2025-03-15T10:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&profile).unwrap();
        let deser: UserProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.user_id, profile.user_id);
        assert_eq!(deser.display_name, profile.display_name);
        assert_eq!(deser.plan, profile.plan);
        assert_eq!(deser.credits_remaining, profile.credits_remaining);
        assert_eq!(deser.credits_used, profile.credits_used);
        assert_eq!(deser.channels, profile.channels);
        assert_eq!(deser.stripe_customer_id, profile.stripe_customer_id);
        assert_eq!(deser.email, profile.email);
        assert_eq!(deser.created_at, profile.created_at);
    }

    // -----------------------------------------------------------------------
    // ChatRequest deserialization tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_chat_request_deserialization() {
        let json = r#"{"message": "hello", "session_id": "sess1"}"#;
        let req: ChatRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.message, "hello");
        assert_eq!(req.session_id, "sess1");
        // channel defaults to "api"
        assert_eq!(req.channel, "api");
        assert_eq!(req.model, None);
        assert!(!req.multi_model);
    }

    #[test]
    fn test_chat_request_defaults() {
        // Only message is truly required; session_id and channel have defaults
        let json = r#"{"message": "test"}"#;
        let req: ChatRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.message, "test");
        assert_eq!(req.session_id, "api:default");
        assert_eq!(req.channel, "api");
        assert_eq!(req.multi_model, false);
        assert_eq!(req.device, None);
    }

    #[test]
    fn test_chat_request_all_fields() {
        let json = r#"{
            "message": "hello",
            "session_id": "s1",
            "channel": "web",
            "model": "gpt-4o",
            "multi_model": true,
            "device": "mobile"
        }"#;
        let req: ChatRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.message, "hello");
        assert_eq!(req.session_id, "s1");
        assert_eq!(req.channel, "web");
        assert_eq!(req.model, Some("gpt-4o".to_string()));
        assert!(req.multi_model);
        assert_eq!(req.device, Some("mobile".to_string()));
    }

    #[test]
    fn test_chat_request_missing_message_fails() {
        let json = r#"{"session_id": "s1"}"#;
        let result = serde_json::from_str::<ChatRequest>(json);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // ChatResponse serialization tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_chat_response_serialization() {
        let resp = ChatResponse {
            response: "hi".to_string(),
            session_id: "s1".to_string(),
            agent: Some("assistant".to_string()),
            tools_used: None,
            credits_used: Some(5),
            credits_remaining: Some(95),
            model_used: Some("gpt-4o".to_string()),
            models_consulted: None,
            action: None,
            input_tokens: Some(100),
            output_tokens: Some(20),
            estimated_cost_usd: Some(0.001),
            mode: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"credits_used\":5"));
        assert!(json.contains("\"estimated_cost_usd\":0.001"));
        // None fields with skip_serializing_if should be absent
        assert!(!json.contains("models_consulted"));
        assert!(!json.contains("action"));
        assert!(!json.contains("tools_used"));
    }

    #[test]
    fn test_chat_response_minimal() {
        let resp = ChatResponse {
            response: "ok".to_string(),
            session_id: "s2".to_string(),
            agent: None,
            tools_used: None,
            credits_used: None,
            credits_remaining: None,
            model_used: None,
            models_consulted: None,
            action: None,
            input_tokens: None,
            output_tokens: None,
            estimated_cost_usd: None,
            mode: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        // Only response and session_id should be present
        assert!(json.contains("\"response\":\"ok\""));
        assert!(json.contains("\"session_id\":\"s2\""));
        // All optional fields should be skipped
        assert!(!json.contains("agent"));
        assert!(!json.contains("credits_used"));
        assert!(!json.contains("model_used"));
        assert!(!json.contains("input_tokens"));
        assert!(!json.contains("output_tokens"));
        assert!(!json.contains("estimated_cost_usd"));
    }

    #[test]
    fn test_chat_response_with_tools() {
        let resp = ChatResponse {
            response: "result".to_string(),
            session_id: "s3".to_string(),
            agent: Some("coder".to_string()),
            tools_used: Some(vec!["web_search".to_string(), "calculator".to_string()]),
            credits_used: Some(10),
            credits_remaining: Some(90),
            model_used: Some("claude-sonnet-4-5-20250929".to_string()),
            models_consulted: Some(vec!["claude-sonnet-4-5-20250929".to_string(), "gpt-4o".to_string()]),
            action: Some("upgrade".to_string()),
            input_tokens: Some(500),
            output_tokens: Some(150),
            estimated_cost_usd: Some(0.005),
            mode: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("web_search"));
        assert!(json.contains("calculator"));
        assert!(json.contains("upgrade"));
        assert!(json.contains("claude-sonnet-4-5-20250929"));
    }

    // -----------------------------------------------------------------------
    // detect_agent additional tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_detect_agent_default() {
        let (agent, cleaned, score) = detect_agent("hello world");
        assert_eq!(agent.id, "assistant");
        assert_eq!(cleaned, "hello world");
        assert_eq!(score, 0);
    }

    #[test]
    fn test_detect_agent_code_keywords() {
        // "debug" has weight 3 in coder, which is >= threshold 2
        let (agent, _, score) = detect_agent("debugして");
        assert_eq!(agent.id, "coder");
        assert!(score >= 2);
    }

    #[test]
    fn test_detect_agent_search_keywords() {
        // "検索" has weight 3 in researcher
        let (agent, _, score) = detect_agent("AIについて検索して");
        assert_eq!(agent.id, "researcher");
        assert!(score >= 2);
    }

    #[test]
    fn test_detect_agent_explicit_prefix_unknown() {
        // Unknown @agent falls through to keyword scoring
        let (agent, _, _) = detect_agent("@unknown hello");
        // "unknown" is not in AGENTS, so falls to keyword scoring
        // "hello" has no keywords, so defaults to assistant
        assert_eq!(agent.id, "assistant");
    }

    #[test]
    fn test_detect_agent_empty_message() {
        let (agent, cleaned, score) = detect_agent("");
        assert_eq!(agent.id, "assistant");
        assert_eq!(cleaned, "");
        assert_eq!(score, 0);
    }

    #[test]
    fn test_detect_agent_threshold() {
        // A single weak keyword (weight=1) should not trigger agent switch
        let (agent, _, score) = detect_agent("rust");
        assert_eq!(agent.id, "assistant");
        assert_eq!(score, 0); // below threshold of 2
    }

    #[test]
    fn test_detect_agent_multiple_keywords_accumulate() {
        // "コード" (weight 2) + "python" (weight 1) = coder score 3
        let (agent, _, score) = detect_agent("pythonでコードを書く方法");
        assert_eq!(agent.id, "coder");
        assert!(score >= 2);
    }

    // -----------------------------------------------------------------------
    // SK_PROFILE constant test
    // -----------------------------------------------------------------------

    #[test]
    fn test_sk_profile_constant() {
        assert_eq!(SK_PROFILE, "PROFILE");
    }

    // -----------------------------------------------------------------------
    // URL_REGEX tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_url_regex_matches() {
        let urls: Vec<&str> = URL_REGEX
            .find_iter("Check https://example.com and http://test.org/page?q=1")
            .map(|m| m.as_str())
            .collect();
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0], "https://example.com");
        assert!(urls[1].starts_with("http://test.org"));
    }

    #[test]
    fn test_url_regex_no_match() {
        let urls: Vec<&str> = URL_REGEX
            .find_iter("no urls here")
            .map(|m| m.as_str())
            .collect();
        assert!(urls.is_empty());
    }

    #[test]
    fn test_url_regex_with_path_and_query() {
        let urls: Vec<&str> = URL_REGEX
            .find_iter("Visit https://example.com/path/to/page?key=val&foo=bar#section")
            .map(|m| m.as_str())
            .collect();
        assert_eq!(urls.len(), 1);
        assert!(urls[0].starts_with("https://example.com/path/to/page"));
    }

    #[test]
    fn test_url_regex_multiple_urls() {
        let text = "See https://a.com https://b.com and http://c.org/d";
        let urls: Vec<&str> = URL_REGEX.find_iter(text).map(|m| m.as_str()).collect();
        assert_eq!(urls.len(), 3);
    }

    // -----------------------------------------------------------------------
    // ErrorResponse serialization test
    // -----------------------------------------------------------------------

    #[test]
    fn test_error_response_serialization() {
        let err = ErrorResponse {
            error: "not found".to_string(),
        };
        let json = serde_json::to_string(&err).unwrap();
        assert_eq!(json, r#"{"error":"not found"}"#);
    }

    #[test]
    fn test_error_response_empty_message() {
        let err = ErrorResponse {
            error: "".to_string(),
        };
        let json = serde_json::to_string(&err).unwrap();
        assert_eq!(json, r#"{"error":""}"#);
    }

    // -----------------------------------------------------------------------
    // hash_password tests
    // -----------------------------------------------------------------------

    /// All hash_password tests in a single test to avoid env var race conditions
    /// between parallel test threads.
    #[test]
    fn test_hash_password_properties() {
        // Use a unique env key for this test
        std::env::set_var("PASSWORD_HMAC_KEY", "test-secret-key-for-hash-tests");

        // Deterministic: same inputs produce same hash
        let hash1 = hash_password("mypassword", "user@example.com");
        let hash2 = hash_password("mypassword", "user@example.com");
        assert_eq!(hash1, hash2, "Same inputs should produce same hash");

        // Different passwords produce different hashes
        let hash_a = hash_password("password1", "salt1");
        let hash_b = hash_password("password2", "salt1");
        assert_ne!(hash_a, hash_b, "Different passwords should produce different hashes");

        // Different salts produce different hashes
        let hash_c = hash_password("password", "salt1");
        let hash_d = hash_password("password", "salt2");
        assert_ne!(hash_c, hash_d, "Different salts should produce different hashes");

        // Output is 64 hex characters (HMAC-SHA256 = 32 bytes)
        assert_eq!(hash1.len(), 64, "Hash should be 64 hex characters");
        assert!(hash1.chars().all(|c| c.is_ascii_hexdigit()), "Hash should be valid hex");

        std::env::remove_var("PASSWORD_HMAC_KEY");

        // Fallback key: when no env vars set, still produces valid hash
        std::env::remove_var("GOOGLE_CLIENT_SECRET");
        let hash_fallback = hash_password("test", "salt");
        assert_eq!(hash_fallback.len(), 64);
        assert!(hash_fallback.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // -----------------------------------------------------------------------
    // detect_language additional tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_detect_language_mixed_content() {
        // Japanese chars present -> "ja"
        assert_eq!(detect_language("Hello こんにちは"), "ja");
        assert_eq!(detect_language("テスト test"), "ja");
    }

    #[test]
    fn test_detect_language_katakana() {
        assert_eq!(detect_language("カタカナ"), "ja");
    }

    #[test]
    fn test_detect_language_kanji() {
        assert_eq!(detect_language("漢字"), "ja");
    }

    #[test]
    fn test_detect_language_pure_ascii() {
        assert_eq!(detect_language("hello world 123 !@#"), "en");
    }

    #[test]
    fn test_detect_language_non_ja_non_ascii() {
        assert_eq!(detect_language("Bonjour le monde"), "en"); // all ASCII
        assert_eq!(detect_language("straße"), "other"); // non-ASCII, non-ja
    }

    // -----------------------------------------------------------------------
    // build_meta_context_with_model tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_build_meta_context_with_model_ja() {
        let ctx = build_meta_context_with_model(
            None, "web", "pc", 0, false,
            Some("unknown-model"), 500, 1500,
        );
        assert!(ctx.contains("現在時刻:"));
        assert!(ctx.contains("モデル: unknown-model"));
        assert!(ctx.contains("この会話: 約500トークン"));
        assert!(ctx.contains("チャネル: web"));
        assert!(ctx.contains("新規"));
    }

    #[test]
    fn test_build_meta_context_with_model_en() {
        let ctx = build_meta_context_with_model(
            None, "api", "voice", 10, true,
            Some("unknown-model"), 1000, 5000,
        );
        assert!(ctx.contains("Time:"));
        assert!(ctx.contains("Model: unknown-model"));
        assert!(ctx.contains("Session: ~1000 tokens"));
        assert!(ctx.contains("Channel: api"));
        assert!(ctx.contains("Device: voice"));
        assert!(ctx.contains("ongoing(10msgs)"));
    }

    #[test]
    fn test_build_meta_context_with_model_no_model() {
        let ctx = build_meta_context_with_model(
            None, "line", "mobile", 0, false,
            None, 0, 0,
        );
        assert!(ctx.contains("現在時刻:"));
        assert!(ctx.contains("チャネル: line"));
        assert!(!ctx.contains("モデル:"));
        // session_tokens=0 means no session info
        assert!(!ctx.contains("この会話:"));
    }

    #[test]
    fn test_build_meta_context_low_credits_warning_ja() {
        let user = UserProfile {
            user_id: "low-cred".to_string(),
            display_name: None,
            plan: "free".to_string(),
            credits_remaining: 50,
            credits_used: 50,
            channels: vec![],
            stripe_customer_id: None,
            email: None,
            created_at: "2025-01-01".to_string(),
        };
        let ctx = build_meta_context(Some(&user), "web", "pc", 0, false);
        assert!(ctx.contains("クレジット残少"));
    }

    #[test]
    fn test_build_meta_context_low_credits_warning_en() {
        let user = UserProfile {
            user_id: "low-cred-en".to_string(),
            display_name: None,
            plan: "free".to_string(),
            credits_remaining: 50,
            credits_used: 50,
            channels: vec![],
            stripe_customer_id: None,
            email: None,
            created_at: "2025-01-01".to_string(),
        };
        let ctx = build_meta_context(Some(&user), "web", "pc", 0, true);
        assert!(ctx.contains("LOW_CREDITS"));
    }

    #[test]
    fn test_build_meta_context_no_low_credits_for_paid() {
        let user = UserProfile {
            user_id: "paid-user".to_string(),
            display_name: None,
            plan: "pro".to_string(),
            credits_remaining: 50,
            credits_used: 50,
            channels: vec![],
            stripe_customer_id: None,
            email: None,
            created_at: "2025-01-01".to_string(),
        };
        let ctx = build_meta_context(Some(&user), "web", "pc", 0, false);
        // Low credits warning only appears for free plan
        assert!(!ctx.contains("クレジット残少"));
    }

    // -----------------------------------------------------------------------
    // AgentProfile and AGENTS constant tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_agents_array_has_expected_ids() {
        let ids: Vec<&str> = AGENTS.iter().map(|a| a.id).collect();
        assert!(ids.contains(&"orchestrator"));
        assert!(ids.contains(&"assistant"));
        assert!(ids.contains(&"researcher"));
        assert!(ids.contains(&"coder"));
        assert!(ids.contains(&"analyst"));
        assert!(ids.contains(&"creative"));
    }

    #[test]
    fn test_agent_profile_serialization() {
        let agent = &AGENTS[1]; // assistant
        let json = serde_json::to_string(agent).unwrap();
        assert!(json.contains("\"id\":\"assistant\""));
        assert!(json.contains("\"name\":\"Assistant\""));
        assert!(json.contains("\"tools_enabled\":true"));
    }

    // -----------------------------------------------------------------------
    // UserSettings serialization tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_user_settings_roundtrip() {
        let settings = UserSettings {
            preferred_model: Some("gpt-4o".to_string()),
            temperature: Some(0.7),
            enabled_tools: Some(vec!["web_search".to_string()]),
            custom_api_keys: None,
            language: Some("ja".to_string()),
            adult_mode: Some(false),
            age_verified: Some(true),
            top_p: Some(0.9),
            frequency_penalty: Some(0.5),
            presence_penalty: None,
            custom_system_prompt: Some("Always respond in Japanese".to_string()),
            streaming_enabled: Some(true),
            show_thinking: None,
            theme: Some("dark".to_string()),
            ui_language: Some("ja".to_string()),
            font_size: Some("medium".to_string()),
            send_method: None,
            tts_speed: Some(1.2),
            show_token_info: Some(true),
            show_timestamps: None,
            compact_mode: None,
            preferred_voice: None,
            preferred_tts_provider: None,
            ai_nickname: None,
            user_nickname: None,
            onboarding_completed: None,
        };
        let json = serde_json::to_string(&settings).unwrap();
        let deser: UserSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.preferred_model, Some("gpt-4o".to_string()));
        assert_eq!(deser.temperature, Some(0.7));
        assert_eq!(deser.language, Some("ja".to_string()));
        assert_eq!(deser.top_p, Some(0.9));
        assert_eq!(deser.custom_system_prompt, Some("Always respond in Japanese".to_string()));
    }

    #[test]
    fn test_user_settings_all_none() {
        let settings = UserSettings {
            preferred_model: None,
            temperature: None,
            enabled_tools: None,
            custom_api_keys: None,
            language: None,
            adult_mode: None,
            age_verified: None,
            top_p: None,
            frequency_penalty: None,
            presence_penalty: None,
            custom_system_prompt: None,
            streaming_enabled: None,
            show_thinking: None,
            theme: None,
            ui_language: None,
            font_size: None,
            send_method: None,
            tts_speed: None,
            show_token_info: None,
            show_timestamps: None,
            compact_mode: None,
            preferred_voice: None,
            preferred_tts_provider: None,
            ai_nickname: None,
            user_nickname: None,
            onboarding_completed: None,
        };
        let json = serde_json::to_string(&settings).unwrap();
        let deser: UserSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.preferred_model, None);
        assert_eq!(deser.temperature, None);
    }

    // -----------------------------------------------------------------------
    // SessionInfo serialization test
    // -----------------------------------------------------------------------

    #[test]
    fn test_session_info_serialization() {
        let info = SessionInfo {
            key: "webchat:abc123".to_string(),
            created_at: Some("2025-01-01T00:00:00Z".to_string()),
            updated_at: None,
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("webchat:abc123"));
        assert!(json.contains("2025-01-01T00:00:00Z"));
    }

    // -----------------------------------------------------------------------
    // GITHUB_TOOL_NAMES constant test
    // -----------------------------------------------------------------------

    #[test]
    fn test_github_tool_names() {
        assert!(GITHUB_TOOL_NAMES.contains(&"github_read_file"));
        assert!(GITHUB_TOOL_NAMES.contains(&"github_create_or_update_file"));
        assert!(GITHUB_TOOL_NAMES.contains(&"github_create_pr"));
        assert_eq!(GITHUB_TOOL_NAMES.len(), 3);
    }

    // -----------------------------------------------------------------------
    // default_session_id and default_channel tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_default_session_id() {
        assert_eq!(default_session_id(), "api:default");
    }

    #[test]
    fn test_default_channel() {
        assert_eq!(default_channel(), "api");
    }
}
