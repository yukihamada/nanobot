use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

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

/// Get the base URL for this instance. Defaults to "https://chatweb.ai".
/// Set BASE_URL env var to customize for self-hosted instances.
pub fn get_base_url() -> String {
    std::env::var("BASE_URL").unwrap_or_else(|_| "https://chatweb.ai".to_string())
}

/// Check if a session key, user ID, or email is an admin.
/// Reads from ADMIN_SESSION_KEYS environment variable (comma-separated).
/// Supports both session keys (e.g. "webchat:xxx") and emails (e.g. "user@example.com").
pub fn is_admin(key: &str) -> bool {
    let keys = std::env::var("ADMIN_SESSION_KEYS").unwrap_or_default();
    keys.split(',').map(|k| k.trim()).any(|k| !k.is_empty() && k == key)
}

/// GitHub tool names that are restricted to admin users.
const GITHUB_TOOL_NAMES: &[&str] = &[
    "github_read_file",
    "github_create_or_update_file",
    "github_create_pr",
];

/// User profile for unified billing and identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub user_id: String,
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
/// Returns combined long-term + today's notes for injection into system prompt.
#[cfg(feature = "dynamodb-backend")]
async fn read_memory_context(
    dynamo: &aws_sdk_dynamodb::Client,
    config_table: &str,
    user_id: &str,
) -> String {
    let mut parts = Vec::new();

    let pk = format!("MEMORY#{}", user_id);
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();

    // Read LONG_TERM and DAILY in parallel
    let (long_term_result, daily_result) = tokio::join!(
        dynamo
            .get_item()
            .table_name(config_table)
            .key("pk", AttributeValue::S(pk.clone()))
            .key("sk", AttributeValue::S("LONG_TERM".to_string()))
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
#[allow(dead_code)]
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

/// Append content to user's daily memory log.
#[cfg(feature = "dynamodb-backend")]
async fn append_daily_memory(
    dynamo: &aws_sdk_dynamodb::Client,
    config_table: &str,
    user_id: &str,
    content: &str,
) {
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
        .key("sk", AttributeValue::S("PROFILE".to_string()))
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
            let stripe_customer_id = item.get("stripe_customer_id").and_then(|v| v.as_s().ok()).cloned();
            let email = item.get("email").and_then(|v| v.as_s().ok()).cloned();
            let created_at = item.get("created_at").and_then(|v| v.as_s().ok()).cloned()
                .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());

            return UserProfile {
                user_id: user_id.to_string(),
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
    let free_credits = crate::service::auth::Plan::Free.monthly_credits();

    let _ = dynamo
        .put_item()
        .table_name(config_table)
        .item("pk", AttributeValue::S(pk))
        .item("sk", AttributeValue::S("PROFILE".to_string()))
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

    // Atomic update with ReturnValue::AllNew to get remaining credits directly
    let remaining = match dynamo
        .update_item()
        .table_name(config_table)
        .key("pk", AttributeValue::S(pk))
        .key("sk", AttributeValue::S("PROFILE".to_string()))
        .update_expression("SET credits_remaining = credits_remaining - :c, credits_used = credits_used + :c, updated_at = :now")
        .expression_attribute_values(":c", AttributeValue::N(credits.to_string()))
        .expression_attribute_values(":now", AttributeValue::S(chrono::Utc::now().to_rfc3339()))
        .return_values(aws_sdk_dynamodb::types::ReturnValue::AllNew)
        .send()
        .await
    {
        Ok(output) => output.attributes
            .and_then(|attrs| attrs.get("credits_remaining").and_then(|v| v.as_n().ok()).and_then(|n| n.parse::<i64>().ok())),
        Err(_) => None,
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

    let _ = dynamo
        .update_item()
        .table_name(config_table)
        .key("pk", AttributeValue::S(pk))
        .key("sk", AttributeValue::S("PROFILE".to_string()))
        .update_expression("SET #p = :plan, stripe_customer_id = :cus, email = :email, credits_remaining = :cr, updated_at = :now")
        .expression_attribute_names("#p", "plan")
        .expression_attribute_values(":plan", AttributeValue::S(plan.to_string()))
        .expression_attribute_values(":cus", AttributeValue::S(stripe_customer_id.to_string()))
        .expression_attribute_values(":email", AttributeValue::S(email.to_string()))
        .expression_attribute_values(":cr", AttributeValue::N(new_credits.to_string()))
        .expression_attribute_values(":now", AttributeValue::S(chrono::Utc::now().to_rfc3339()))
        .send()
        .await;

    info!("Linked Stripe customer {} to user {} with plan {}", stripe_customer_id, user_id, plan);
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
}

const AGENTS: &[AgentProfile] = &[
    AgentProfile {
        id: "orchestrator",
        name: "Orchestrator",
        description: "Routes tasks to the best specialist agent",
        system_prompt: "",  // handled specially
        tools_enabled: false,
        icon: "brain",
    },
    AgentProfile {
        id: "assistant",
        name: "Assistant",
        description: "General-purpose AI agent — OpenClaw-derived, Rust-native",
        system_prompt: "# nanobot — AI Agent System\n\n\
             あなたは nanobot — OpenClaw（旧Clawdbot/Moltbot）の派生プロジェクトをRustで完全に書き直した\
             高速・高信頼AIエージェントシステムです。AWS Lambda (ARM64) 上で並行実行され、\
             <2秒の応答速度を実現します。全てオープンソースです: github.com/yukihamada\n\n\
             ## SOUL（性格）\n\
             - 好奇心旺盛で行動力がある。聞かれたら即座に動く。\n\
             - 親しみやすく、ユーモアも交えるが、技術的には正確で妥協しない。\n\
             - OpenClawの海賊猫の精神を受け継ぎ、困難に立ち向かう勇気がある。\n\
             - 「できません」より「こうすればできます」を提案する。\n\
             - ユーザーの言語に自動で合わせる（日本語で聞かれたら日本語、英語なら英語）。\n\
             - 不確実な情報は正直に伝える。推測と事実を区別する。\n\n\
             ## できること\n\
             - **ウェブ検索・リサーチ**: 最新ニュース、価格比較、情報収集をリアルタイム実行\n\
             - **データ分析・計算**: 数値計算、通貨換算、統計分析\n\
             - **天気予報**: 世界中の天気情報を取得\n\
             - **文章作成・翻訳**: コピーライティング、多言語翻訳、要約\n\
             - **プログラミング支援**: コード作成、デバッグ、設計アドバイス（Rust得意）\n\
             - **Googleカレンダー連携**: 予定の確認・作成（連携済みの場合）\n\
             - **Gmail連携**: メールの検索・閲覧・送信（連携済みの場合）\n\
             - **マルチチャネル**: LINE, Telegram, Discord, Slack, Teams, WhatsApp, Facebook — 14+チャネル対応\n\n\
             ## チャネル連携の案内\n\
             ユーザーがLINE・Telegram等との連携について聞いたら、以下を案内してください:\n\
             - **LINE**: 友だち追加 → https://line.me/R/oaMessage/@619jcqqh/ （またはWeb画面のLINEボタンをタップ）\n\
             - **Telegram**: @chatweb_ai_bot を検索して /start （またはWeb画面のTelegramボタンをタップ）\n\
             - **連携方法**: Web画面の入力欄横にある LINE/TG ボタンを押すと、QRコードが表示されます。スマホならそのまま開いて自動連携。\n\
             - **/link コマンド**: どのチャネルでも `/link` と送信するとリンクコードが発行されます。別チャネルで `/link <コード>` を送ると連携完了。\n\
             - 一度連携すると、どのチャネルでも同じ会話・クレジット・記憶が共有されます。\n\n\
             ## yukihamada.jpサービス連携\n\
             以下のサービスとネイティブに連携します:\n\
             - **chatweb.ai**: 日本語 voice-first AIアシスタント（このシステムの日本語フロントエンド）\n\
             - **teai.io**: 開発者向けAIエージェントプラットフォーム（英語メイン）\n\
             - **ElioChat (elio.love)**: 完全オフライン・オンデバイスAI（iPhone）\n\
             - **kouzou**: 木造住宅耐震診断\n\
             - **taishin**: AI写真解析による耐震診断\n\
             - **TOTONO**: 10-in-1 ビジネスOS\n\
             - **BANTO**: 声で請求書作成（建設業向け）\n\n\
             ## 行動規範\n\
             - 事実を求められたら、まずweb_searchで最新情報を検索する。記憶だけで答えない。\n\
             - 回答は構造化する（箇条書き、見出し、表を活用）。長文より簡潔さを優先。\n\
             - 情報源があればURLを明示する。\n\
             - ツールを積極的に使う。持っている能力は出し惜しみしない。\n\n\
             ## ツール\n\
             - web_search: ウェブ検索。事実確認・最新情報取得。\n\
             - web_fetch: URL内容取得。検索結果の詳細確認。\n\
             - calculator: 計算、通貨換算、数式評価。\n\
             - weather: 天気・予報の取得。\n\
             - google_calendar: Googleカレンダー（連携済みの場合）。\n\
             - gmail: メール検索・閲覧・送信（連携済みの場合）。",
        tools_enabled: true,
        icon: "chat",
    },
    AgentProfile {
        id: "researcher",
        name: "Researcher",
        description: "Web research, fact-checking, data gathering",
        system_prompt: "あなたは nanobot のリサーチ専門エージェントです。\n\
             OpenClaw派生のRust製AIエージェントシステムの調査機能を担当します。\n\n\
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
    },
    AgentProfile {
        id: "coder",
        name: "Coder",
        description: "Code writing, debugging, architecture design",
        system_prompt: "あなたは nanobot のプログラミング専門エージェントです。\n\
             nanobot自体がRust (axum) で書かれたAWSLambda上のエージェントシステムであり、\n\
             あなたはそのコーディング能力を体現する存在です。\n\n\
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
    },
    AgentProfile {
        id: "analyst",
        name: "Analyst",
        description: "Data analysis, business insights, financial analysis",
        system_prompt: "あなたは nanobot のデータ分析専門エージェントです。\n\
             OpenClaw派生のRust製AIエージェントシステムの分析機能を担当します。\n\n\
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
    },
    AgentProfile {
        id: "creative",
        name: "Creative",
        description: "Writing, copywriting, brainstorming, translation",
        system_prompt: "あなたは nanobot のクリエイティブ専門エージェントです。\n\
             OpenClawの海賊猫精神を受け継ぎ、大胆で魅力的なコンテンツを生み出します。\n\n\
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
    },
];

/// Detect which agent to use from message text.
/// Supports @agent prefix or auto-routing via orchestrator.
fn detect_agent(text: &str) -> (&'static AgentProfile, String) {
    let trimmed = text.trim();

    // Check for @agent prefix
    if trimmed.starts_with('@') {
        let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
        let agent_id = &parts[0][1..]; // strip @
        let remaining = if parts.len() > 1 { parts[1] } else { "" };

        for agent in AGENTS {
            if agent.id == agent_id {
                return (agent, remaining.to_string());
            }
        }
    }

    // Auto-route based on keywords
    let lower = trimmed.to_lowercase();

    // Code-related keywords
    if lower.contains("コード") || lower.contains("プログラム") || lower.contains("バグ")
        || lower.contains("code") || lower.contains("debug") || lower.contains("function")
        || lower.contains("rust") || lower.contains("python") || lower.contains("javascript")
        || lower.contains("api") || lower.contains("実装") || lower.contains("エラー")
    {
        return (&AGENTS[3], trimmed.to_string()); // coder
    }

    // Research keywords (includes calendar/gmail/schedule queries)
    if lower.contains("調べ") || lower.contains("検索") || lower.contains("リサーチ")
        || lower.contains("search") || lower.contains("research") || lower.contains("比較")
        || lower.contains("最新") || lower.contains("ニュース") || lower.contains("天気")
        || lower.contains("weather")
        || lower.contains("カレンダー") || lower.contains("calendar") || lower.contains("予定")
        || lower.contains("スケジュール") || lower.contains("schedule")
        || lower.contains("メール") || lower.contains("email") || lower.contains("gmail")
        || lower.contains("送信") || lower.contains("受信")
    {
        return (&AGENTS[2], trimmed.to_string()); // researcher
    }

    // Analysis keywords
    if lower.contains("分析") || lower.contains("データ") || lower.contains("計算")
        || lower.contains("analy") || lower.contains("calculate") || lower.contains("統計")
        || lower.contains("グラフ") || lower.contains("予測")
    {
        return (&AGENTS[4], trimmed.to_string()); // analyst
    }

    // Creative keywords (note: "メール" moved to researcher for gmail tool access)
    if lower.contains("書いて") || lower.contains("翻訳")
        || lower.contains("write") || lower.contains("translat") || lower.contains("コピー")
        || lower.contains("キャッチ") || lower.contains("ブログ") || lower.contains("文章")
    {
        return (&AGENTS[5], trimmed.to_string()); // creative
    }

    // Default: general assistant
    (&AGENTS[1], trimmed.to_string())
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
}

/// User settings stored in DynamoDB
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSettings {
    pub preferred_model: Option<String>,
    pub temperature: Option<f64>,
    pub enabled_tools: Option<Vec<String>>,
    pub custom_api_keys: Option<std::collections::HashMap<String, String>>,
    pub language: Option<String>,
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
}

/// Request body for email registration.
#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
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
}

/// Request body for email verification code.
#[derive(Debug, Deserialize)]
pub struct VerifyRequest {
    pub email: String,
    pub code: String,
    pub session_id: Option<String>,
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
        // Billing
        // Settings
        .route("/api/v1/settings/{id}", get(handle_get_settings))
        .route("/api/v1/settings/{id}", post(handle_update_settings))
        .route("/settings", get(handle_settings_page))
        .route("/api/v1/billing/checkout", post(handle_billing_checkout))
        .route("/api/v1/billing/portal", get(handle_billing_portal))
        // Coupon
        .route("/api/v1/coupon/validate", post(handle_coupon_validate))
        .route("/api/v1/coupon/redeem", post(handle_coupon_redeem))
        // SSE streaming chat
        .route("/api/v1/chat/stream", post(handle_chat_stream))
        // Multi-model explore (SSE — all models, progressive)
        .route("/api/v1/chat/explore", post(handle_chat_explore))
        // Webhooks
        .route("/webhooks/line", post(handle_line_webhook))
        .route("/webhooks/telegram", post(handle_telegram_webhook))
        .route("/webhooks/facebook", get(handle_facebook_verify))
        .route("/webhooks/facebook", post(handle_facebook_webhook))
        .route("/webhooks/teams", post(handle_teams_webhook))
        .route("/webhooks/google_chat", post(handle_google_chat_webhook))
        .route("/webhooks/zalo", post(handle_zalo_webhook))
        .route("/webhooks/feishu", post(handle_feishu_webhook))
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
        .route("/api/v1/conversations/{id}/messages", get(handle_get_conversation_messages))
        .route("/api/v1/conversations/{id}", delete(handle_delete_conversation))
        .route("/api/v1/conversations/{id}/share", post(handle_share_conversation))
        .route("/api/v1/conversations/{id}/share", delete(handle_revoke_share))
        .route("/api/v1/shared/{hash}", get(handle_get_shared))
        .route("/c/{hash}", get(handle_shared_page))
        // Sync (ElioChat ↔ chatweb.ai)
        .route("/api/v1/sync/conversations", get(handle_sync_list_conversations))
        .route("/api/v1/sync/conversations/{id}", get(handle_sync_get_conversation))
        .route("/api/v1/sync/push", post(handle_sync_push))
        // Cron (Scheduled Tasks)
        .route("/api/v1/cron", get(handle_cron_list))
        .route("/api/v1/cron", post(handle_cron_create))
        .route("/api/v1/cron/{id}", axum::routing::put(handle_cron_update))
        .route("/api/v1/cron/{id}", delete(handle_cron_delete))
        // Speech (TTS)
        .route("/api/v1/speech/synthesize", post(handle_speech_synthesize))
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
        // Link code generation for QR flow
        .route("/api/v1/link/generate", post(handle_link_generate))
        // MCP endpoint
        .route("/mcp", post(handle_mcp))
        // AI agent friendly
        .route("/robots.txt", get(handle_robots_txt))
        .route("/llms.txt", get(handle_llms_txt))
        .route("/llms-full.txt", get(handle_llms_full_txt))
        .route("/.well-known/ai-plugin.json", get(handle_ai_plugin))
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
                ]),
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
        });
    }

    info!("Chat request: session={}, message={}", req.session_id, req.message);

    // Resolve unified session key
    let session_key = {
        #[cfg(feature = "dynamodb-backend")]
        {
            if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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
                if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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
            });
        }
    };

    // Phase B: Parallel initialization — fetch user, memory, and settings concurrently
    #[cfg(feature = "dynamodb-backend")]
    let (cached_user, parallel_memory, parallel_settings) = {
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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
                return Json(ChatResponse {
                    response: "クレジットが不足しています。プランをアップグレードしてください。\nYou've run out of credits. Please upgrade your plan at /pricing".to_string(),
                    session_id: req.session_id,
                    agent: None,
                    tools_used: None,
                    credits_used: Some(0),
                    credits_remaining: Some(0),
                    model_used: None,
                    models_consulted: None,
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
    let (agent, clean_message) = detect_agent(&req.message);
    info!("Agent selected: {} for message", agent.id);

    // Build conversation with session history — include current date + memory in system prompt
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();

    // Use memory context from parallel initialization
    let memory_context = parallel_memory;

    // Detect teai.io host for developer-focused prompt context
    let chat_host = headers.get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let is_teai = chat_host.contains("teai.io");

    let base_prompt = if is_teai {
        format!(
            "You are Tei — the developer-facing persona of nanobot, an OpenClaw-derived AI agent system \
             rewritten in Rust, running on AWS Lambda (ARM64) with parallel execution and <2s response time.\n\
             All open source: github.com/yukihamada\n\n\
             ## SOUL\n\
             - Technical, precise, and concise. You speak code fluently.\n\
             - Inherited the pirate-cat spirit of OpenClaw — bold, direct, and fearless.\n\
             - Prefer English unless the user writes in another language.\n\
             - Focus on: code generation, debugging, architecture, API design, DevOps.\n\
             - Use code blocks with language tags. Be direct and actionable.\n\n\
             ## Native Service Integrations (yukihamada.jp)\n\
             - teai.io: This platform. Developer-focused AI agent.\n\
             - chatweb.ai: Japanese voice-first AI assistant (same backend).\n\
             - ElioChat: On-device offline AI for iPhone.\n\n\
             {}", agent.system_prompt
        )
    } else {
        agent.system_prompt.to_string()
    };

    let system_prompt = if memory_context.is_empty() {
        format!("{}\n\n今日の日付: {}", base_prompt, today)
    } else {
        format!("{}\n\n今日の日付: {}\n\n---\n{}", base_prompt, today, memory_context)
    };
    let mut messages = vec![
        Message::system(&system_prompt),
    ];

    // Get session history (refresh to pick up messages from other channels)
    {
        let mut sessions = state.sessions.lock().await;
        let session = sessions.refresh(&session_key);
        let history = session.get_history(20);
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

    // Use user settings from parallel initialization
    let user_settings: Option<UserSettings> = parallel_settings;

    // Use model from: request > user settings > web-best-model > global default
    let default_model = state.config.agents.defaults.model.clone();
    let model = req.model
        .as_deref()
        .or(user_settings.as_ref().and_then(|s| s.preferred_model.as_deref()))
        .unwrap_or_else(|| {
            // Web channel gets the best model when no explicit preference is set
            if req.channel == "web" || req.channel.starts_with("webchat") {
                "claude-sonnet-4-5-20250929"
            } else {
                &default_model
            }
        });
    let model = model.to_string();
    let max_tokens = state.config.agents.defaults.max_tokens;
    let temperature = user_settings.as_ref()
        .and_then(|s| s.temperature)
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
                    let mut total_credits: i64 = 0;
                    let mut last_remaining: Option<i64> = None;
                    #[cfg(feature = "dynamodb-backend")]
                    {
                        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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
                    });
                }
                Err(e) => {
                    tracing::error!("Parallel multi-model race failed: {}, falling back to single", e);
                    // Fall through to normal single-model path
                }
            }
        }
    }

    let mut total_credits_used: i64 = 0;
    let mut last_remaining_credits: Option<i64> = None;

    let (response_text, tools_used) = match active_provider.chat(&messages, tools_ref, &model, max_tokens, temperature).await {
        Ok(completion) => {
            info!("LLM response: finish_reason={:?}, tool_calls={}, content_len={}",
                completion.finish_reason, completion.tool_calls.len(),
                completion.content.as_ref().map(|c| c.len()).unwrap_or(0));
            // Deduct credits after successful LLM call
            #[cfg(feature = "dynamodb-backend")]
            {
                if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
                    let (credits, remaining) = deduct_credits(
                        dynamo, table, &session_key, &model,
                        completion.usage.prompt_tokens, completion.usage.completion_tokens,
                    ).await;
                    total_credits_used += credits;
                    if remaining.is_some() { last_remaining_credits = remaining; }
                    tracing::debug!("Deducted {} credits for user {}", credits, session_key);
                }
            }

            // Handle tool calls: multi-iteration agentic loop (up to max_iterations rounds)
            let mut current = completion;
            let mut conversation = messages.clone();
            let mut all_tool_results: Vec<(String, String, String)> = Vec::new();

            // Determine max iterations based on user plan
            let max_iterations: usize = {
                #[cfg(feature = "dynamodb-backend")]
                {
                    match cached_user.as_ref().map(|u| u.plan.as_str()) {
                        Some("pro") | Some("enterprise") => 5,
                        Some("starter") => 3,
                        _ => 1, // free plan: single turn (backward compat)
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
                        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
                            let user_pk = format!("USER#{}", session_key);
                            if let Ok(output) = dynamo.get_item()
                                .table_name(table.as_str())
                                .key("pk", AttributeValue::S(user_pk))
                                .key("sk", AttributeValue::S("PROFILE".to_string()))
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
                    if name == "code_execute" || name == "file_read" || name == "file_write" || name == "file_list" {
                        args.insert("_sandbox_dir".to_string(), serde_json::Value::String(sandbox_dir_ref.to_string()));
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
                    if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
                        let now = chrono::Utc::now();
                        for (_, tool_name, tool_result) in &tool_results {
                            let usage_pk = format!("USAGE#{}#{}", session_key, now.format("%Y%m%d"));
                            let usage_sk = format!("{}#{}", now.to_rfc3339(), tool_name);
                            let result_preview = if tool_result.len() > 200 {
                                format!("{}...", &tool_result[..200])
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
                        #[cfg(feature = "dynamodb-backend")]
                        {
                            if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
                                let (credits, remaining) = deduct_credits(dynamo, table, &session_key, &model,
                                    resp.usage.prompt_tokens, resp.usage.completion_tokens).await;
                                total_credits_used += credits;
                                if remaining.is_some() { last_remaining_credits = remaining; }
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
            tracing::error!("LLM error: {}", e);
            (format!("Error: {}", e), None)
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

    // Auto-save to daily memory log (fire-and-forget)
    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
            let dynamo = dynamo.clone();
            let table = table.clone();
            let sk = session_key.clone();
            let user_msg = req.message.clone();
            let bot_msg = response_text.clone();
            tokio::spawn(async move {
                let summary = format!("- Q: {} → A: {}",
                    if user_msg.len() > 80 { format!("{}...", &user_msg[..80]) } else { user_msg },
                    if bot_msg.len() > 120 { format!("{}...", &bot_msg[..120]) } else { bot_msg },
                );
                append_daily_memory(&dynamo, &table, &sk, &summary).await;
            });
        }
    }

    // Use remaining credits from deduct_credits (no extra DynamoDB call needed)
    let remaining_credits: Option<i64> = last_remaining_credits;

    Json(ChatResponse {
        response: response_text,
        session_id: req.session_id,
        agent: Some(agent.id.to_string()),
        tools_used,
        credits_used: if total_credits_used > 0 { Some(total_credits_used) } else { None },
        credits_remaining: remaining_credits,
        model_used: Some(model.clone()),
        models_consulted: None,
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
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
            // Resolve unified user key
            let user_key = resolve_session_key(dynamo, table, session_id).await;

            // Query DEVICE# records for this user
            let resp = dynamo
                .query()
                .table_name(table.as_str())
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
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
            let user_key = resolve_session_key(dynamo, table, &req.session_id).await;
            let now = chrono::Utc::now();

            let mut item_builder = dynamo
                .put_item()
                .table_name(table.as_str())
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
) -> impl IntoResponse {
    // Resolve unified session key for linked channels
    let session_key = {
        #[cfg(feature = "dynamodb-backend")]
        {
            if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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
    let linked_channels = {
        #[cfg(feature = "dynamodb-backend")]
        {
            if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
                get_linked_channels(dynamo, table, &session_key).await
            } else {
                vec![]
            }
        }
        #[cfg(not(feature = "dynamodb-backend"))]
        {
            Vec::<String>::new()
        }
    };

    let mut sessions = state.sessions.lock().await;
    // Force reload from storage to get latest messages from all channels
    let session = sessions.refresh(&session_key);
    let history = session.get_full_history(100);
    Json(serde_json::json!({
        "key": id,
        "resolved_key": session_key,
        "messages": history,
        "message_count": history.len(),
        "linked_channels": linked_channels,
    }))
}

/// Query DynamoDB for linked channels associated with a user.
#[cfg(feature = "dynamodb-backend")]
async fn get_linked_channels(
    dynamo: &aws_sdk_dynamodb::Client,
    table: &str,
    session_key: &str,
) -> Vec<String> {
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

    let mut channels = vec!["web".to_string()];

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
                if key.starts_with("line:") && !channels.contains(&"line".to_string()) {
                    channels.push("line".to_string());
                } else if key.starts_with("telegram:") && !channels.contains(&"telegram".to_string()) {
                    channels.push("telegram".to_string());
                }
            }
        }
    }

    // Also check the direct query response
    if let Ok(output) = resp {
        for item in output.items.unwrap_or_default() {
            if let Some(uid) = item.get("user_id").and_then(|v| v.as_s().ok()) {
                if uid != session_key {
                    // This link record exists — there's a linked channel
                    if uid.starts_with("line:") && !channels.contains(&"line".to_string()) {
                        channels.push("line".to_string());
                    } else if uid.starts_with("telegram:") && !channels.contains(&"telegram".to_string()) {
                        channels.push("telegram".to_string());
                    }
                }
            }
        }
    }

    channels
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

    let user_id = user_id_from_token.as_deref()
        .or_else(|| headers.get("x-user-id").and_then(|v| v.to_str().ok()))
        .unwrap_or("anonymous");

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
            let resolved = resolve_session_key(dynamo, table, user_id).await;
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
                let welcome = "Ahoy! 友だち追加ありがとう！\n\n\
                    僕は nanobot — OpenClaw派生のAIエージェントだよ。Rustで動く高速AIで、何でも聞いてね。\n\n\
                    まず教えて:\n\
                    - 僕のことなんて呼ぶ？（デフォルト: nanobot）\n\
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
                        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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
                            if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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
                                    "あなたはChatWeb（chatweb.ai）、高速で賢いAIアシスタントです。\
                                     あなたの名前は「ChatWeb」です。OpenCLAWなど他のサービス名を名乗らないでください。\
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
                                        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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

        if let Some(ref sid) = web_session_id {
            // Auto-link via deep link
            #[cfg(feature = "dynamodb-backend")]
            if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
                auto_link_session(dynamo, table, &channel_key, sid, &state.sessions).await;
                let reply = "Link complete! Your Web and Telegram conversations are now synced.\nYou can continue the same conversation on any channel.";
                let client = reqwest::Client::new();
                if let Err(e) = TelegramChannel::send_message_static(&client, token, &chat_id, reply).await {
                    tracing::error!("Failed to send Telegram link reply: {}", e);
                }
                return StatusCode::OK;
            }
        }

        let welcome = "Ahoy! Welcome aboard! 🏴‍☠️\n\n\
            I'm nanobot — an OpenClaw-derived AI agent rewritten in Rust. Fast, reliable, and fully open source.\n\n\
            Let's set up:\n\
            - What should I call you?\n\
            - Preferred tone? (casual / professional / pirate 🏴‍☠️)\n\n\
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
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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
            if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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
                    "あなたはChatWeb（chatweb.ai）、高速で賢いAIアシスタントです。\
                     あなたの名前は「ChatWeb」です。OpenCLAWなど他のサービス名を名乗らないでください。\
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
                        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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
                        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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

                let system_prompt = "あなたは nanobot — OpenClaw派生のRust製AIエージェントです。Facebook Messengerで会話しています。300文字以内で簡潔に回答してください。オープンソース: github.com/yukihamada";
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
                            if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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
            if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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

    let system_prompt = "あなたは nanobot — Rust製AIエージェントです。Microsoft Teamsで会話しています。300文字以内で簡潔に回答してください。";
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
                if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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
            if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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

    let system_prompt = "あなたは nanobot — Rust製AIエージェントです。Google Chatで会話しています。300文字以内で簡潔に回答してください。";
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
                if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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
            if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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

    let system_prompt = "あなたは nanobot — Rust製AIエージェントです。Zaloで会話しています。300文字以内で簡潔に回答してください。";
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
                if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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
            if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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

    let system_prompt = "あなたは nanobot — Rust製AIエージェントです。Feishu/Larkで会話しています。300文字以内で簡潔に回答してください。";
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
                if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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
            if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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

    let system_prompt = "あなたは nanobot — Rust製AIエージェントです。WhatsAppで会話しています。300文字以内で簡潔に回答してください。";
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
                if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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
            if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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
    let (stream_user, stream_settings) = {
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
            let (user, settings) = tokio::join!(
                get_or_create_user(dynamo, table, &session_key),
                get_user_settings(dynamo, table, &session_key)
            );
            (Some(user), Some(settings))
        } else {
            (None, None)
        }
    };
    #[cfg(not(feature = "dynamodb-backend"))]
    let (stream_user, stream_settings): (Option<UserProfile>, Option<UserSettings>) = (None, None);

    // Check credits (using cached user)
    #[cfg(feature = "dynamodb-backend")]
    {
        if let Some(ref user) = stream_user {
            if user.credits_remaining <= 0 {
                let err_stream = stream::once(async {
                    Ok::<_, Infallible>(Event::default().data(
                        serde_json::json!({"type":"error","content":"No credits remaining"}).to_string()
                    ))
                });
                return Sse::new(err_stream).into_response();
            }
        }
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

    // Build messages — host-aware system prompt
    let stream_host = headers.get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let stream_system_prompt = if stream_host.contains("teai.io") {
        "You are Tei — the developer-facing persona of nanobot, an OpenClaw-derived AI agent \
         rewritten in Rust, running on AWS Lambda. All open source: github.com/yukihamada\n\
         Be technical, precise, and concise. Use code blocks with language tags. \
         Prefer English unless the user writes in another language. \
         Focus on code generation, debugging, architecture, and technical problem-solving. \
         Native integrations: teai.io, chatweb.ai, ElioChat, kouzou, taishin, TOTONO, BANTO."
    } else {
        "あなたは nanobot — OpenClaw派生のRust製AIエージェントシステムです。\
         chatweb.aiのフロントエンドを通じてWebで会話しています。\
         ユーザーの質問に正確かつ詳しく回答してください。全てオープンソース: github.com/yukihamada"
    };
    let mut messages = vec![Message::system(stream_system_prompt)];

    {
        let mut sessions = state.sessions.lock().await;
        let session = sessions.refresh(&session_key);
        let history = session.get_history(20);
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
    messages.push(Message::user(&req.message));

    // Use user settings from parallel initialization
    let user_settings: Option<UserSettings> = stream_settings;

    let default_model = state.config.agents.defaults.model.clone();
    let model = req.model.as_deref()
        .or(user_settings.as_ref().and_then(|s| s.preferred_model.as_deref()))
        .unwrap_or_else(|| {
            if req.channel == "web" || req.channel.starts_with("webchat") {
                "claude-sonnet-4-5-20250929"
            } else {
                &default_model
            }
        }).to_string();
    let max_tokens = state.config.agents.defaults.max_tokens;
    let temperature = user_settings.as_ref()
        .and_then(|s| s.temperature)
        .unwrap_or(state.config.agents.defaults.temperature);

    // Agentic SSE stream: supports multi-iteration tool calling with progress events.
    // Collects all SSE events into a Vec (API Gateway v2 compatible — no async_stream).
    let req_message = req.message.clone();
    let req_session_id = req.session_id.clone();
    let state_clone = state.clone();
    let session_key_clone = session_key.clone();

    // Get tools definitions for the stream handler
    let tools: Vec<serde_json::Value> = state.tool_registry.get_definitions();

    // Determine max iterations based on user plan
    let max_iterations: usize = {
        #[cfg(feature = "dynamodb-backend")]
        {
            match stream_user.as_ref().map(|u| u.plan.as_str()) {
                Some("pro") | Some("enterprise") => 5,
                Some("starter") => 3,
                _ => 1,
            }
        }
        #[cfg(not(feature = "dynamodb-backend"))]
        { 5 }
    };

    let response_stream = stream::once(async move {
        // Collect all SSE events into a Vec, then join as multi-line SSE
        // (API Gateway v2 compatible — futures::stream::once pattern)
        let mut events: Vec<serde_json::Value> = Vec::new();

        // Start event
        events.push(serde_json::json!({"type":"start","session_id": req_session_id}));

        let tools_ref = if tools.is_empty() { None } else { Some(&tools[..]) };

        // First LLM call (with tools for agentic mode)
        let first_result = provider.chat(&messages, tools_ref, &model, max_tokens, temperature).await;

        match first_result {
            Ok(completion) => {
                let mut total_credits_used: i64 = 0;
                let mut last_remaining: Option<i64> = None;

                // Deduct credits for first call
                #[cfg(feature = "dynamodb-backend")]
                {
                    if let (Some(ref dynamo), Some(ref table)) = (&state_clone.dynamo_client, &state_clone.config_table) {
                        let (credits, remaining) = deduct_credits(dynamo, table, &session_key_clone, &model,
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

                    // Emit tool_start events
                    for tc in &tool_calls_to_run {
                        events.push(serde_json::json!({
                            "type": "tool_start",
                            "tool": tc.name,
                            "iteration": iteration,
                        }));
                    }

                    // Execute tool calls in parallel
                    let registry = &state_clone.tool_registry;
                    let futures_vec: Vec<_> = tool_calls_to_run.iter().map(|tc| {
                        let name = tc.name.clone();
                        let mut args = tc.arguments.clone();
                        let id = tc.id.clone();
                        if name == "code_execute" || name == "file_read" || name == "file_write" || name == "file_list" {
                            args.insert("_sandbox_dir".to_string(), serde_json::Value::String(sandbox_dir.clone()));
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

                    // Emit tool_result events
                    for (_, name, result) in &tool_results {
                        all_tools_used.push(name.clone());
                        let preview_end = result.char_indices().nth(500).map(|(i, _)| i).unwrap_or(result.len());
                        events.push(serde_json::json!({
                            "type": "tool_result",
                            "tool": name,
                            "result": &result[..preview_end],
                            "iteration": iteration,
                        }));
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

                    // Emit thinking event
                    events.push(serde_json::json!({
                        "type": "thinking",
                        "iteration": iteration,
                    }));

                    // Follow-up LLM call: pass tools if more iterations remain
                    let follow_up_tools = if iteration < max_iterations {
                        Some(&tools[..])
                    } else {
                        None
                    };

                    match provider.chat(&conversation, follow_up_tools, &model, max_tokens, temperature).await {
                        Ok(resp) => {
                            #[cfg(feature = "dynamodb-backend")]
                            {
                                if let (Some(ref dynamo), Some(ref table)) = (&state_clone.dynamo_client, &state_clone.config_table) {
                                    let (credits, remaining) = deduct_credits(dynamo, table, &session_key_clone, &model,
                                        resp.usage.prompt_tokens, resp.usage.completion_tokens).await;
                                    total_credits_used += credits;
                                    if remaining.is_some() { last_remaining = remaining; }
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
                }

                let response_text = current.content.unwrap_or_default();

                // Save to session
                {
                    let mut sessions = state_clone.sessions.lock().await;
                    let session = sessions.get_or_create(&session_key_clone);
                    session.add_message("user", &req_message);
                    session.add_message("assistant", &response_text);
                    sessions.save_by_key(&session_key_clone);
                }

                // Content event (final answer)
                events.push(serde_json::json!({
                    "type": "content",
                    "content": response_text,
                    "credits_remaining": last_remaining,
                    "credits_used": if total_credits_used > 0 { Some(total_credits_used) } else { None::<i64> },
                    "tools_used": if all_tools_used.is_empty() { None } else { Some(&all_tools_used) },
                    "iterations": iteration,
                }));
            }
            Err(e) => {
                tracing::error!("LLM stream error: {}", e);
                events.push(serde_json::json!({"type":"error","content": format!("Error: {}", e)}));
            }
        }

        // Done event
        events.push(serde_json::json!({"type":"done"}));

        // Emit all events as a single SSE data payload (API Gateway v2 compatible)
        // Client parses the JSON array to reconstruct individual events
        Ok::<_, Infallible>(Event::default().data(
            serde_json::to_string(&events).unwrap_or_else(|_| "[]".to_string())
        ))
    });

    Sse::new(response_stream)
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
            if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
            let user = get_or_create_user(dynamo, table, &session_key).await;
            if user.credits_remaining <= 0 {
                let err_stream = futures::stream::once(async {
                    Ok::<_, Infallible>(Event::default()
                        .event("error")
                        .data(serde_json::json!({"error": "No credits remaining"}).to_string()))
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

    // Build messages
    let mut messages = vec![Message::system(
        "あなたは nanobot — OpenClaw派生のRust製AIエージェントシステムです。\
         ユーザーの質問に正確かつ詳しく回答してください。"
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
    let user_msg = match level {
        1 => format!(
            "{}\n\n上記の質問について、ステップバイステップで考えてから回答してください。",
            req.message
        ),
        2 => format!(
            "あなたはこの分野の専門家です。以下の質問について、\
             まず前提条件を整理し、複数の観点から分析し、\
             最終的な結論を根拠とともに示してください。\n\n質問: {}",
            req.message
        ),
        _ => req.message.clone(),
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
        let mut total_credits: i64 = 0;
        let mut last_remaining: Option<i64> = None;

        #[cfg(feature = "dynamodb-backend")]
        {
            if let (Some(ref dynamo), Some(ref table)) = (&state_clone.dynamo_client, &state_clone.config_table) {
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

/// POST /api/v1/billing/checkout — Create Stripe Checkout session
async fn handle_billing_checkout(
    State(_state): State<Arc<AppState>>,
    Json(req): Json<CheckoutRequest>,
) -> impl IntoResponse {
    let price_id = match req.plan.to_lowercase().as_str() {
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

    let success_url = std::env::var("CHECKOUT_SUCCESS_URL")
        .unwrap_or_else(|_| "https://nanobot.page/success".to_string());
    let cancel_url = std::env::var("CHECKOUT_CANCEL_URL")
        .unwrap_or_else(|_| "https://nanobot.page/pricing".to_string());

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "checkout_url": format!(
                "https://checkout.stripe.com/pay/{}?success_url={}&cancel_url={}",
                price_id, success_url, cancel_url
            ),
            "price_id": price_id,
            "plan": req.plan,
        })),
    )
}

/// GET /api/v1/billing/portal — Get Stripe billing portal URL
async fn handle_billing_portal(
    State(_state): State<Arc<AppState>>,
) -> impl IntoResponse {
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

            // On checkout.session.completed, create tenant in DynamoDB
            #[cfg(feature = "dynamodb-backend")]
            if event_type == "checkout.session.completed" {
                if let (Some(ref client), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
                    let customer_id = event
                        .pointer("/data/object/customer")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let customer_email = event
                        .pointer("/data/object/customer_details/email")
                        .or_else(|| event.pointer("/data/object/customer_email"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    // Determine plan from price
                    let plan_name = if let Some(price_id) = event
                        .pointer("/data/object/display_items/0/price/id")
                        .or_else(|| event.pointer("/data/object/line_items/data/0/price/id"))
                        .and_then(|v| v.as_str())
                    {
                        match crate::service::stripe::price_to_plan(price_id) {
                            Some(p) => p.to_string(),
                            None => "starter".to_string(),
                        }
                    } else {
                        "starter".to_string()
                    };

                    let api_key = crate::service::auth::generate_api_key("nb_live");
                    let api_key_hash = crate::service::auth::hash_api_key(&api_key);
                    let now = chrono::Utc::now().to_rfc3339();

                    use aws_sdk_dynamodb::types::AttributeValue;
                    let put_result = client
                        .put_item()
                        .table_name(table)
                        .item("pk", AttributeValue::S(format!("TENANT#{}", customer_id)))
                        .item("sk", AttributeValue::S("CONFIG".to_string()))
                        .item("tenant_id", AttributeValue::S(customer_id.to_string()))
                        .item("email", AttributeValue::S(customer_email.to_string()))
                        .item("plan", AttributeValue::S(plan_name.clone()))
                        .item("api_key_hash", AttributeValue::S(api_key_hash))
                        .item("created_at", AttributeValue::S(now.clone()))
                        .item("updated_at", AttributeValue::S(now))
                        .item("status", AttributeValue::S("active".to_string()))
                        .send()
                        .await;

                    match put_result {
                        Ok(_) => info!("Tenant created in DynamoDB: customer={}, email={}", customer_id, customer_email),
                        Err(e) => tracing::error!("Failed to create tenant in DynamoDB: {}", e),
                    }

                    // Log the API key (in production, send via email)
                    info!("API key generated for {}: {} (hash: ...)", customer_email, &api_key[..12]);

                    // Try to find existing user by email or create new
                    let user_id = format!("user:{}", uuid::Uuid::new_v4());
                    let _ = get_or_create_user(client, table, &user_id).await;
                    link_stripe_to_user(client, table, &user_id, customer_id, customer_email, &plan_name).await;
                }
            }

            // Handle subscription updates (plan changes)
            #[cfg(feature = "dynamodb-backend")]
            if event_type == "customer.subscription.updated" || event_type == "invoice.paid" {
                let customer_id = event
                    .pointer("/data/object/customer")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                if !customer_id.is_empty() {
                    info!("Subscription event for customer {}: {}", customer_id, event_type);
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
    if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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
    if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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

        // 3. Check if already redeemed (prevent double-use)
        let redeem_check = dynamo
            .get_item()
            .table_name(table)
            .key("pk", AttributeValue::S(format!("REDEEM#{}#{}", resolved_user, code)))
            .key("sk", AttributeValue::S("INFO".to_string()))
            .send()
            .await;

        if let Ok(ref output) = redeem_check {
            if output.item.is_some() {
                return (axum::http::StatusCode::BAD_REQUEST, Json(serde_json::json!({
                    "error": "Coupon already redeemed",
                    "error_ja": "このクーポンは既に使用済みです"
                }))).into_response();
            }
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
            .key("sk", AttributeValue::S("PROFILE".to_string()))
            .update_expression("SET credits_remaining = credits_remaining + :c, #p = :plan, coupon_code = :coupon, coupon_expires = :exp, updated_at = :now")
            .expression_attribute_names("#p", "plan")
            .expression_attribute_values(":c", AttributeValue::N(grant_credits.to_string()))
            .expression_attribute_values(":plan", AttributeValue::S(grant_plan.clone()))
            .expression_attribute_values(":coupon", AttributeValue::S(code.clone()))
            .expression_attribute_values(":exp", AttributeValue::S(expires_at.clone()))
            .expression_attribute_values(":now", AttributeValue::S(now.clone()))
            .send()
            .await;

        // 6. Record redemption (prevent re-use)
        let ttl = (chrono::Utc::now() + chrono::Duration::days(grant_days + 30)).timestamp();
        let _ = dynamo
            .put_item()
            .table_name(table)
            .item("pk", AttributeValue::S(format!("REDEEM#{}#{}", resolved_user, code)))
            .item("sk", AttributeValue::S("INFO".to_string()))
            .item("user_id", AttributeValue::S(resolved_user.clone()))
            .item("code", AttributeValue::S(code.clone()))
            .item("grant_credits", AttributeValue::N(grant_credits.to_string()))
            .item("grant_plan", AttributeValue::S(grant_plan.clone()))
            .item("redeemed_at", AttributeValue::S(now.clone()))
            .item("ttl", AttributeValue::N(ttl.to_string()))
            .send()
            .await;

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
            "message": "Coupon redeemed successfully!",
            "message_ja": format!("クーポン適用完了！{}クレジット付与、{}日間{}プラン", grant_credits, grant_days, grant_plan),
        })).into_response();
    }

    #[cfg(not(feature = "dynamodb-backend"))]
    let _ = (&state, &headers);

    (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
        "error": "Coupon system not available"
    }))).into_response()
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
            if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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
    let host = headers.get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if host.starts_with("api.") {
        // Serve API docs for api.chatweb.ai / api.teai.io
        axum::response::Html(include_str!("../../../../web/api-docs.html"))
    } else if host.contains("teai.io") {
        // Serve teai.io developer-focused landing page
        axum::response::Html(include_str!("../../../../web/teai-index.html"))
    } else {
        axum::response::Html(include_str!("../../../../web/index.html"))
    }
}

/// GET /pricing — Pricing page (host-based routing)
async fn handle_pricing(headers: axum::http::HeaderMap) -> impl IntoResponse {
    let host = headers.get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
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
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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
        if let (Some(ref dynamo), Some(ref config_table)) = (&state.dynamo_client, &state.config_table) {
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
                    .expression_attribute_values(":sk", AttributeValue::S("PROFILE".to_string()))
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

/// GET /og.svg — OGP image (host-based routing)
async fn handle_og_svg(headers: axum::http::HeaderMap) -> impl IntoResponse {
    let host = headers.get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
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
async fn handle_health() -> impl IntoResponse {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: crate::VERSION.to_string(),
    })
}

// ---------------------------------------------------------------------------
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

        let (dynamo, table) = match (&state.dynamo_client, &state.config_table) {
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

/// POST /api/v1/results — Save a playground result for sharing
async fn handle_save_result(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let id = super::commands::generate_share_hash();

    #[cfg(feature = "dynamodb-backend")]
    if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
        let now = chrono::Utc::now().to_rfc3339();
        let body_str = serde_json::to_string(&body).unwrap_or_default();
        let ttl = (chrono::Utc::now().timestamp() + 86400 * 30).to_string(); // 30 days

        let _ = dynamo
            .put_item()
            .table_name(table.as_str())
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
    if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
        let result = dynamo
            .get_item()
            .table_name(table.as_str())
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
        let key = std::env::var("ANTHROPIC_API_KEY").unwrap();
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
        let key = std::env::var("GROQ_API_KEY").unwrap();
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

    // --- Kimi / Moonshot ---
    let kimi_key = std::env::var("KIMI_API_KEY")
        .or_else(|_| std::env::var("MOONSHOT_API_KEY"))
        .ok()
        .filter(|k| !k.is_empty());
    if let Some(key) = kimi_key {
        let c = client.clone();
        handles.push(tokio::spawn(async move {
            let start = std::time::Instant::now();
            let res = c.get("https://api.moonshot.cn/v1/models")
                .header("Authorization", format!("Bearer {key}"))
                .send().await;
            let ms = start.elapsed().as_millis() as u64;
            match res {
                Ok(r) if r.status().is_success() => serde_json::json!({
                    "name": "Kimi", "status": "ok", "latency_ms": ms
                }),
                Ok(r) => serde_json::json!({
                    "name": "Kimi", "status": "error",
                    "latency_ms": ms, "detail": format!("HTTP {}", r.status())
                }),
                Err(e) => serde_json::json!({
                    "name": "Kimi", "status": "error",
                    "latency_ms": ms, "detail": e.to_string()
                }),
            }
        }));
    } else {
        handles.push(tokio::spawn(async {
            serde_json::json!({"name": "Kimi", "status": "not_configured"})
        }));
    }

    // --- DynamoDB ---
    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(ref ddb), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
            let ddb = ddb.clone();
            let table = table.clone();
            handles.push(tokio::spawn(async move {
                let start = std::time::Instant::now();
                // Use get_item on a non-existent key — requires only read permissions
                let res = ddb.get_item()
                    .table_name(&table)
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
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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
        },
        "session_id": id,
    }))
}

/// POST /api/v1/settings/{id} — Update user settings (requires auth)
async fn handle_update_settings(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<UpdateSettingsRequest>,
) -> impl IntoResponse {
    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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

            save_user_settings(dynamo, table, &session_key, &req).await;
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
            return UserSettings { preferred_model, temperature, enabled_tools, custom_api_keys, language };
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
    }
}

/// Mask API key for safe display (show first 4 chars + ****)
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
        .unwrap_or_else(|_| "chatweb-default-key".to_string());
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
    let host = headers.get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
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
    let host = headers.get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
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
    let refresh_token = token_data.get("refresh_token").and_then(|v| v.as_str()).map(|s| s.to_string());

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
    let display_name = userinfo.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let session_id = params.state.unwrap_or_default();

    if google_sub.is_empty() {
        return axum::response::Redirect::temporary("/?auth=error&reason=no_sub");
    }

    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
            let now = chrono::Utc::now().to_rfc3339();

            // Check if GOOGLE#{sub} already exists → get existing user_id
            let google_pk = format!("GOOGLE#{}", google_sub);
            let existing = dynamo
                .get_item()
                .table_name(table.as_str())
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
                    let uid = if !session_id.is_empty() {
                        resolve_session_key(dynamo, table, &session_id).await
                    } else {
                        format!("user:{}", uuid::Uuid::new_v4())
                    };

                    // Create GOOGLE#{sub} → USER_MAP
                    let _ = dynamo
                        .put_item()
                        .table_name(table.as_str())
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
                .table_name(table.as_str())
                .key("pk", AttributeValue::S(user_pk))
                .key("sk", AttributeValue::S("PROFILE".to_string()))
                .expression_attribute_values(":email", AttributeValue::S(email.clone()))
                .expression_attribute_values(":name", AttributeValue::S(display_name.clone()))
                .expression_attribute_values(":gid", AttributeValue::S(google_sub.clone()))
                .expression_attribute_values(":auth", AttributeValue::S("google".to_string()))
                .expression_attribute_values(":now", AttributeValue::S(now.clone()));

            // Store refresh token if provided (for Calendar/Gmail API access)
            if let Some(ref rt) = refresh_token {
                update_expr.push_str(", google_refresh_token = :rt");
                update_req = update_req.expression_attribute_values(":rt", AttributeValue::S(rt.clone()));
            }
            let _ = update_req.update_expression(update_expr).send().await;

            // Link session if provided
            if !session_id.is_empty() {
                let link_pk = format!("LINK#{}", session_id);
                let _ = dynamo
                    .put_item()
                    .table_name(table.as_str())
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
                .table_name(table.as_str())
                .item("pk", AttributeValue::S(format!("AUTH#{}", auth_token)))
                .item("sk", AttributeValue::S("TOKEN".to_string()))
                .item("user_id", AttributeValue::S(user_id.clone()))
                .item("email", AttributeValue::S(email.clone()))
                .item("display_name", AttributeValue::S(display_name.clone()))
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
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
            let auth_pk = format!("AUTH#{}", token);
            if let Ok(output) = dynamo
                .get_item()
                .table_name(table.as_str())
                .key("pk", AttributeValue::S(auth_pk))
                .key("sk", AttributeValue::S("TOKEN".to_string()))
                .send()
                .await
            {
                if let Some(item) = output.item {
                    let user_id = item.get("user_id").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                    let email = item.get("email").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                    let display_name = item.get("display_name").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();

                    // Look up user profile for credits info
                    let session_key = item.get("session_key").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                    let lookup_id = if !session_key.is_empty() { &session_key } else { &user_id };
                    let user_profile = get_or_create_user(dynamo, table, lookup_id).await;

                    return Json(serde_json::json!({
                        "authenticated": true,
                        "user_id": user_id,
                        "email": email,
                        "display_name": display_name,
                        "credits_remaining": user_profile.credits_remaining,
                        "credits_used": user_profile.credits_used,
                        "plan": user_profile.plan,
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
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
            // Rate limit: 3 registrations per minute per email
            if !check_rate_limit(dynamo, table, &format!("register:{}", email), 3).await {
                return (StatusCode::TOO_MANY_REQUESTS, Json(serde_json::json!({ "error": "Too many requests. Please try again later." })));
            }

            let email_pk = format!("EMAIL#{}", email);

            // Check if email already registered
            if let Ok(output) = dynamo
                .get_item()
                .table_name(table.as_str())
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
                .table_name(table.as_str())
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

            // Update profile with email and auth method
            let user_pk = format!("USER#{}", user_id);
            let _ = dynamo
                .update_item()
                .table_name(table.as_str())
                .key("pk", AttributeValue::S(user_pk))
                .key("sk", AttributeValue::S("PROFILE".to_string()))
                .update_expression("SET email = :email, auth_method = :auth, updated_at = :now")
                .expression_attribute_values(":email", AttributeValue::S(email.clone()))
                .expression_attribute_values(":auth", AttributeValue::S("email".to_string()))
                .expression_attribute_values(":now", AttributeValue::S(now.clone()))
                .send()
                .await;

            // Create auth token
            let auth_token = uuid::Uuid::new_v4().to_string();
            let ttl = (chrono::Utc::now().timestamp() + 30 * 24 * 3600).to_string();
            let _ = dynamo
                .put_item()
                .table_name(table.as_str())
                .item("pk", AttributeValue::S(format!("AUTH#{}", auth_token)))
                .item("sk", AttributeValue::S("TOKEN".to_string()))
                .item("user_id", AttributeValue::S(user_id.clone()))
                .item("email", AttributeValue::S(email.clone()))
                .item("display_name", AttributeValue::S(email.clone()))
                .item("created_at", AttributeValue::S(now))
                .item("ttl", AttributeValue::N(ttl))
                .send()
                .await;

            emit_audit_log(dynamo.clone(), table.clone(), "register", &user_id, &email, "email_register");

            return (StatusCode::OK, Json(serde_json::json!({
                "ok": true,
                "token": auth_token,
                "user_id": user_id,
                "email": email,
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
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
            // Rate limit: 5 login attempts per minute per email
            if !check_rate_limit(dynamo, table, &format!("login:{}", email), 5).await {
                return (StatusCode::TOO_MANY_REQUESTS, Json(serde_json::json!({ "error": "Too many requests. Please try again later." })));
            }

            let email_pk = format!("EMAIL#{}", email);

            // Lookup credentials
            let cred_result = dynamo
                .get_item()
                .table_name(table.as_str())
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
                                .table_name(table.as_str())
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
                        .table_name(table.as_str())
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
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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
                let _ = dynamo
                    .put_item()
                    .table_name(table.as_str())
                    .item("pk", AttributeValue::S(verify_pk))
                    .item("sk", AttributeValue::S("CODE".to_string()))
                    .item("code", AttributeValue::S(code.clone()))
                    .item("attempts", AttributeValue::N("0".to_string()))
                    .item("session_id", AttributeValue::S(req.session_id.clone().unwrap_or_default()))
                    .item("ttl", AttributeValue::N(ttl))
                    .item("created_at", AttributeValue::S(chrono::Utc::now().to_rfc3339()))
                    .send()
                    .await;

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
                .table_name(table.as_str())
                .key("pk", AttributeValue::S(email_pk.clone()))
                .key("sk", AttributeValue::S("CREDENTIALS".to_string()))
                .send()
                .await;

            let user_id = if let Ok(output) = &existing {
                if let Some(item) = &output.item {
                    item.get("user_id").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default()
                } else {
                    let new_user_id = format!("user:{}", uuid::Uuid::new_v4());
                    let now = chrono::Utc::now().to_rfc3339();

                    let _ = dynamo
                        .put_item()
                        .table_name(table.as_str())
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
                        .table_name(table.as_str())
                        .key("pk", AttributeValue::S(user_pk))
                        .key("sk", AttributeValue::S("PROFILE".to_string()))
                        .update_expression("SET email = :email, auth_method = :auth, updated_at = :now")
                        .expression_attribute_values(":email", AttributeValue::S(email.clone()))
                        .expression_attribute_values(":auth", AttributeValue::S("email_passwordless".to_string()))
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
                        .table_name(table.as_str())
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
                .table_name(table.as_str())
                .item("pk", AttributeValue::S(format!("AUTH#{}", auth_token)))
                .item("sk", AttributeValue::S("TOKEN".to_string()))
                .item("user_id", AttributeValue::S(user_id.clone()))
                .item("email", AttributeValue::S(email.clone()))
                .item("display_name", AttributeValue::S(email.clone()))
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
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
            let verify_pk = format!("VERIFY#{}", email);

            // Look up the stored code
            let stored = dynamo
                .get_item()
                .table_name(table.as_str())
                .key("pk", AttributeValue::S(verify_pk.clone()))
                .key("sk", AttributeValue::S("CODE".to_string()))
                .send()
                .await;

            let (stored_code, stored_session_id, attempts) = match stored {
                Ok(output) => {
                    if let Some(item) = output.item {
                        let sc = item.get("code").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                        let sid = item.get("session_id").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                        let att = item.get("attempts").and_then(|v| v.as_n().ok())
                            .and_then(|n| n.parse::<i64>().ok()).unwrap_or(0);
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
                        (sc, sid, att)
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
                .table_name(table.as_str())
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
                .table_name(table.as_str())
                .key("pk", AttributeValue::S(verify_pk))
                .key("sk", AttributeValue::S("CODE".to_string()))
                .send()
                .await;

            // Get or create user
            let email_pk = format!("EMAIL#{}", email);
            let existing = dynamo
                .get_item()
                .table_name(table.as_str())
                .key("pk", AttributeValue::S(email_pk.clone()))
                .key("sk", AttributeValue::S("CREDENTIALS".to_string()))
                .send()
                .await;

            let user_id = if let Ok(output) = &existing {
                if let Some(item) = &output.item {
                    item.get("user_id").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default()
                } else {
                    let new_user_id = format!("user:{}", uuid::Uuid::new_v4());
                    let now = chrono::Utc::now().to_rfc3339();

                    let _ = dynamo
                        .put_item()
                        .table_name(table.as_str())
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
                        .table_name(table.as_str())
                        .key("pk", AttributeValue::S(user_pk))
                        .key("sk", AttributeValue::S("PROFILE".to_string()))
                        .update_expression("SET email = :email, auth_method = :auth, updated_at = :now")
                        .expression_attribute_values(":email", AttributeValue::S(email.clone()))
                        .expression_attribute_values(":auth", AttributeValue::S("email_verified".to_string()))
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
                    .table_name(table.as_str())
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
                .table_name(table.as_str())
                .item("pk", AttributeValue::S(format!("AUTH#{}", auth_token)))
                .item("sk", AttributeValue::S("TOKEN".to_string()))
                .item("user_id", AttributeValue::S(user_id.clone()))
                .item("email", AttributeValue::S(email.clone()))
                .item("display_name", AttributeValue::S(email.clone()))
                .item("created_at", AttributeValue::S(now))
                .item("ttl", AttributeValue::N(ttl))
                .send()
                .await;

            emit_audit_log(dynamo.clone(), table.clone(), "email_verified", &user_id, &email, "email_code_verified");

            return (StatusCode::OK, Json(serde_json::json!({
                "ok": true,
                "token": auth_token,
                "user_id": user_id,
                "email": email,
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
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
            // Resolve user from token
            let user_id = resolve_user_from_token(dynamo, table, &token).await;
            if user_id.is_empty() {
                return Json(serde_json::json!({ "conversations": [], "error": "Not authenticated" }));
            }

            let user_pk = format!("USER#{}", user_id);
            let resp = dynamo
                .query()
                .table_name(table.as_str())
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
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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
                .table_name(table.as_str())
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
                .table_name(table.as_str())
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
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
            let user_id = resolve_user_from_token(dynamo, table, &token).await;
            if user_id.is_empty() {
                return Json(serde_json::json!({ "messages": [], "error": "Not authenticated" }));
            }

            // Get the session_id from the conversation record
            let user_pk = format!("USER#{}", user_id);
            let conv_resp = dynamo
                .get_item()
                .table_name(table.as_str())
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

            // Get messages from session store
            let mut store = state.sessions.lock().await;
            let session = store.get_or_create(&session_id);
            let messages: Vec<serde_json::Value> = session.messages.iter().map(|m| {
                serde_json::json!({
                    "role": m.role,
                    "content": m.content,
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
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
            let user_id = resolve_user_from_token(dynamo, table, &token).await;
            if user_id.is_empty() {
                return Json(serde_json::json!({ "error": "Not authenticated" }));
            }

            // Check user is paid (free users can't delete)
            let profile = get_or_create_user(dynamo, table, &user_id).await;
            if profile.plan == "free" {
                return Json(serde_json::json!({ "error": "Conversation deletion is available for paid plans" }));
            }

            let user_pk = format!("USER#{}", user_id);
            let _ = dynamo
                .delete_item()
                .table_name(table.as_str())
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
    Path(_hash): Path<String>,
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
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
            // Look up the share record
            let resp = dynamo
                .get_item()
                .table_name(table.as_str())
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
                .table_name(table.as_str())
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
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
            let user_id = match auth_user_id(&state, &headers).await {
                Some(uid) => uid,
                None => return Json(serde_json::json!({ "error": "Not authenticated" })),
            };

            // Verify user owns this conversation
            let user_pk = format!("USER#{}", user_id);
            let conv_resp = dynamo
                .get_item()
                .table_name(table.as_str())
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
                .table_name(table.as_str())
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
                            .table_name(table.as_str())
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
                .table_name(table.as_str())
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
                .table_name(table.as_str())
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
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
            let user_id = match auth_user_id(&state, &headers).await {
                Some(uid) => uid,
                None => return Json(serde_json::json!({ "error": "Not authenticated" })),
            };

            // Verify user owns this conversation
            let user_pk = format!("USER#{}", user_id);
            let conv_resp = dynamo
                .get_item()
                .table_name(table.as_str())
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
                .table_name(table.as_str())
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
                        .table_name(table.as_str())
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

/// Helper: extract user_id from Bearer token via DynamoDB lookup
#[cfg(feature = "dynamodb-backend")]
async fn auth_user_id(state: &AppState, headers: &axum::http::HeaderMap) -> Option<String> {
    let token = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_start_matches("Bearer ").to_string())
        .unwrap_or_default();
    if token.is_empty() { return None; }
    if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
        let auth_pk = format!("AUTH#{}", token);
        if let Ok(output) = dynamo
            .get_item()
            .table_name(table.as_str())
            .key("pk", AttributeValue::S(auth_pk))
            .key("sk", AttributeValue::S("TOKEN".to_string()))
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
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
            let pk = format!("USER#{}", user_id);
            if let Ok(output) = dynamo
                .query()
                .table_name(table.as_str())
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

        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
            // Store under user
            let _ = dynamo
                .put_item()
                .table_name(table.as_str())
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
                .table_name(table.as_str())
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
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
            // Get the key first to find the api_key for reverse lookup cleanup
            let pk = format!("USER#{}", user_id);
            let sk = format!("APIKEY#{}", key_id);
            if let Ok(get_output) = dynamo
                .get_item()
                .table_name(table.as_str())
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
                            .table_name(table.as_str())
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
                .table_name(table.as_str())
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
// TTS (Text-to-Speech) via OpenAI API
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SpeechRequest {
    text: String,
    #[serde(default = "default_tts_voice")]
    voice: String,
    #[serde(default = "default_tts_speed")]
    speed: f64,
    session_id: Option<String>,
}

fn default_tts_voice() -> String { "nova".to_string() }
fn default_tts_speed() -> f64 { 1.0 }

/// POST /api/v1/speech/synthesize — Convert text to speech via OpenAI TTS API
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

    // Get OpenAI API key from env
    let api_key = match std::env::var("OPENAI_API_KEY").or_else(|_| std::env::var("OPENAI_API_KEYS").map(|keys| {
        keys.split(',').next().unwrap_or("").trim().to_string()
    })) {
        Ok(k) if !k.is_empty() => k,
        _ => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                b"{ \"error\": \"TTS service not configured\" }".to_vec(),
            );
        }
    };

    // Check credits if user is authenticated
    #[cfg(feature = "dynamodb-backend")]
    {
        let user_id = auth_user_id(&state, &headers).await;
        if let Some(ref uid) = user_id {
            if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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

    // Call OpenAI TTS API
    let client = reqwest::Client::new();
    let tts_result = client
        .post("https://api.openai.com/v1/audio/speech")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&serde_json::json!({
            "model": "tts-1",
            "input": req.text,
            "voice": req.voice,
            "speed": req.speed,
            "response_format": "mp3",
        }))
        .send()
        .await;

    match tts_result {
        Ok(resp) if resp.status().is_success() => {
            let audio_bytes = resp.bytes().await.unwrap_or_default().to_vec();

            // Deduct credits: 1 credit per 100 characters
            #[cfg(feature = "dynamodb-backend")]
            {
                let user_id = auth_user_id(&state, &headers).await;
                if let Some(ref uid) = user_id {
                    if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
                        let tts_credits = std::cmp::max(1, (req.text.len() as i64) / 100);
                        let pk = format!("USER#{}", uid);
                        let _ = dynamo
                            .update_item()
                            .table_name(table)
                            .key("pk", AttributeValue::S(pk))
                            .key("sk", AttributeValue::S("PROFILE".to_string()))
                            .update_expression("SET credits_remaining = credits_remaining - :c, credits_used = credits_used + :c, updated_at = :now")
                            .expression_attribute_values(":c", AttributeValue::N(tts_credits.to_string()))
                            .expression_attribute_values(":now", AttributeValue::S(chrono::Utc::now().to_rfc3339()))
                            .send()
                            .await;
                    }
                }
            }

            (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "audio/mpeg")],
                audio_bytes,
            )
        }
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            tracing::error!("OpenAI TTS error: status={}, body={}", status, body);
            (
                StatusCode::BAD_GATEWAY,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                format!("{{ \"error\": \"TTS API error: {}\" }}", status).into_bytes(),
            )
        }
        Err(e) => {
            tracing::error!("OpenAI TTS request failed: {}", e);
            (
                StatusCode::BAD_GATEWAY,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                b"{ \"error\": \"TTS request failed\" }".to_vec(),
            )
        }
    }
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
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
            let user_id = resolve_user_from_token(dynamo, table, &token).await;
            if user_id.is_empty() {
                return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "Invalid token" })));
            }

            let user_pk = format!("USER#{}", user_id);

            // Build query — optionally filter by updated_at >= since
            let mut query = dynamo
                .query()
                .table_name(table.as_str())
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
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
            let user_id = resolve_user_from_token(dynamo, table, &token).await;
            if user_id.is_empty() {
                return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "Invalid token" })));
            }

            // Look up the CONV record to get title and session_id
            let user_pk = format!("USER#{}", user_id);
            let conv_resp = dynamo
                .get_item()
                .table_name(table.as_str())
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
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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
                    .table_name(table.as_str())
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
                    .table_name(table.as_str())
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
struct CronCreateRequest {
    name: String,
    message: String,
    schedule: CronScheduleInput,
    channel: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CronScheduleInput {
    every_minutes: Option<u64>,
    cron: Option<String>,
    at: Option<String>,
}

#[derive(Debug, Deserialize)]
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
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
            let user_id = resolve_user_from_token(dynamo, table, &token).await;
            if user_id.is_empty() {
                return Json(serde_json::json!({ "jobs": [], "error": "Not authenticated" }));
            }
            let user_pk = format!("CRON#{}", user_id);
            let resp = dynamo
                .query()
                .table_name(table.as_str())
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
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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
                .table_name(table.as_str())
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

            match dynamo.put_item().table_name(table.as_str()).set_item(Some(item)).send().await {
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
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
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
                .table_name(table.as_str())
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
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
            let user_id = resolve_user_from_token(dynamo, table, &token).await;
            if user_id.is_empty() {
                return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "Not authenticated" }))).into_response();
            }
            let user_pk = format!("CRON#{}", user_id);
            let sk = format!("JOB#{}", id);
            match dynamo.delete_item()
                .table_name(table.as_str())
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
