use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use axum::{
    extract::{Path, State},
    http::{self, StatusCode},
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use tower_http::cors::{Any, CorsLayer};
use tower_http::compression::CompressionLayer;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::info;

use crate::channel::line::LineChannel;
use crate::channel::telegram::TelegramChannel;
use crate::channel::{is_allowed};
use crate::config::Config;
use crate::provider::{self, LlmProvider};
use crate::session::store::SessionStore;
use crate::types::Message;
#[cfg(feature = "stripe")]
use crate::service::stripe::{process_webhook_event, verify_webhook_signature};

#[cfg(feature = "dynamodb-backend")]
use aws_sdk_dynamodb::types::AttributeValue;

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
    /// Unified tool registry (built-in + MCP tools)
    pub tool_registry: crate::service::integrations::ToolRegistry,
    /// Per-user concurrent request tracker: session_key -> active count
    pub concurrent_requests: dashmap::DashMap<String, AtomicU32>,
    #[cfg(feature = "dynamodb-backend")]
    pub dynamo_client: Option<aws_sdk_dynamodb::Client>,
    #[cfg(feature = "dynamodb-backend")]
    pub config_table: Option<String>,
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
        let lb_provider = provider::LoadBalancedProvider::from_env()
            .map(|lb| Arc::new(lb) as Arc<dyn LlmProvider>);

        // Create tool registry with built-in tools
        let tool_registry = crate::service::integrations::ToolRegistry::with_builtins();

        Self {
            config,
            sessions: Mutex::new(sessions),
            provider,
            lb_provider,
            tool_registry,
            concurrent_requests: dashmap::DashMap::new(),
            #[cfg(feature = "dynamodb-backend")]
            dynamo_client: None,
            #[cfg(feature = "dynamodb-backend")]
            config_table: None,
        }
    }

    /// Get the best provider for a request. Prefers load-balanced if available.
    pub fn get_provider(&self) -> Option<&Arc<dyn LlmProvider>> {
        self.lb_provider.as_ref().or(self.provider.as_ref())
    }
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

/// Result of processing a `/link` command.
#[cfg(feature = "dynamodb-backend")]
enum LinkResult {
    /// A new link code was generated; reply with this message.
    CodeGenerated(String),
    /// Channels were successfully linked.
    Linked(String),
    /// An error occurred.
    Error(String),
}

/// Handle `/link` commands.
/// - `/link` (no args) → generate a 6-char code, store in DynamoDB with 5-min TTL.
/// - `/link CODE` → look up the code, link the two channels, merge sessions.
#[cfg(feature = "dynamodb-backend")]
async fn handle_link_command(
    dynamo: &aws_sdk_dynamodb::Client,
    config_table: &str,
    channel_key: &str,
    code_arg: Option<&str>,
    sessions: &Mutex<Box<dyn SessionStore>>,
) -> LinkResult {
    match code_arg {
        None => {
            // Generate a 6-char alphanumeric code from UUID
            let raw = uuid::Uuid::new_v4().to_string().replace('-', "");
            let code: String = raw.chars().filter(|c| c.is_ascii_alphanumeric()).take(6).collect();
            let code = code.to_uppercase();

            let ttl = (chrono::Utc::now().timestamp() + 1800).to_string(); // 30 min

            let result = dynamo
                .put_item()
                .table_name(config_table)
                .item("pk", AttributeValue::S(format!("LINKCODE#{}", code)))
                .item("sk", AttributeValue::S("PENDING".to_string()))
                .item("channel_key", AttributeValue::S(channel_key.to_string()))
                .item("ttl", AttributeValue::N(ttl))
                .send()
                .await;

            match result {
                Ok(_) => LinkResult::CodeGenerated(
                    format!("リンクコード: {}\n別のチャネル（LINE/Telegram/Web）で「/link {}」と送信してください。\n有効期限: 30分", code, code)
                ),
                Err(e) => {
                    tracing::error!("Failed to store link code: {}", e);
                    LinkResult::Error("リンクコードの生成に失敗しました。".to_string())
                }
            }
        }
        Some(code) => {
            let code = code.trim().to_uppercase();

            // Look up the pending code
            let resp = dynamo
                .get_item()
                .table_name(config_table)
                .key("pk", AttributeValue::S(format!("LINKCODE#{}", code)))
                .key("sk", AttributeValue::S("PENDING".to_string()))
                .send()
                .await;

            let other_channel_key = match resp {
                Ok(output) => {
                    match output.item {
                        Some(item) => {
                            // Check TTL
                            if let Some(ttl_val) = item.get("ttl").and_then(|v| v.as_n().ok()) {
                                if let Ok(ttl) = ttl_val.parse::<i64>() {
                                    if chrono::Utc::now().timestamp() > ttl {
                                        return LinkResult::Error("リンクコードの有効期限が切れています。もう一度 /link で新しいコードを生成してください。".to_string());
                                    }
                                }
                            }
                            match item.get("channel_key").and_then(|v| v.as_s().ok()) {
                                Some(k) => k.clone(),
                                None => return LinkResult::Error("無効なリンクコードです。".to_string()),
                            }
                        }
                        None => return LinkResult::Error("リンクコードが見つかりません。正しいコードか確認してください。".to_string()),
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to look up link code: {}", e);
                    return LinkResult::Error("リンクコードの確認に失敗しました。".to_string());
                }
            };

            if other_channel_key == channel_key {
                return LinkResult::Error("同じチャネルではリンクできません。別のチャネルからコードを入力してください。".to_string());
            }

            // Determine unified user_id: check if either channel already has one
            let existing_a = resolve_session_key(dynamo, config_table, &other_channel_key).await;
            let existing_b = resolve_session_key(dynamo, config_table, channel_key).await;

            let user_id = if existing_a.starts_with("user:") {
                existing_a.clone()
            } else if existing_b.starts_with("user:") {
                existing_b.clone()
            } else {
                format!("user:{}", uuid::Uuid::new_v4())
            };

            // Write LINK# records for both channels
            let now = chrono::Utc::now().to_rfc3339();
            for ck in [&other_channel_key, &channel_key.to_string()] {
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

            // Merge session histories into the unified session
            {
                let mut store = sessions.lock().await;
                // Collect messages from both old sessions
                let old_key_a = if existing_a.starts_with("user:") { existing_a.clone() } else { other_channel_key.clone() };
                let old_key_b = if existing_b.starts_with("user:") { existing_b.clone() } else { channel_key.to_string() };

                let mut all_msgs: Vec<(String, String)> = Vec::new();

                // Gather from old session A
                {
                    let session_a = store.get_or_create(&old_key_a);
                    for m in &session_a.messages {
                        all_msgs.push((m.role.clone(), m.content.clone()));
                    }
                }
                // Gather from old session B (only if different key)
                if old_key_b != old_key_a {
                    let session_b = store.get_or_create(&old_key_b);
                    for m in &session_b.messages {
                        all_msgs.push((m.role.clone(), m.content.clone()));
                    }
                }

                // Write into unified session
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

            // Delete the used link code
            let _ = dynamo
                .delete_item()
                .table_name(config_table)
                .key("pk", AttributeValue::S(format!("LINKCODE#{}", code)))
                .key("sk", AttributeValue::S("PENDING".to_string()))
                .send()
                .await;

            info!("Channels linked: {} <-> {} => {}", other_channel_key, channel_key, user_id);
            LinkResult::Linked("リンク完了！これからどのチャネルでも同じ会話を続けられます。".to_string())
        }
    }
}

/// Parse a `/link` command from text. Returns `Some(None)` for bare `/link`,
/// `Some(Some(code))` for `/link CODE`, or `None` if not a link command.
#[cfg(feature = "dynamodb-backend")]
fn parse_link_command(text: &str) -> Option<Option<&str>> {
    let trimmed = text.trim();
    // Exact "/link" command
    if trimmed == "/link" {
        return Some(None);
    }
    // "/link CODE" at the start
    if let Some(rest) = trimmed.strip_prefix("/link ") {
        let code = rest.trim();
        if !code.is_empty() {
            // Extract just the 6-char code (first word)
            let first_word = code.split_whitespace().next().unwrap_or(code);
            return Some(Some(first_word));
        }
        return Some(None);
    }
    // Search for "/link XXXXXX" anywhere in the text (for copy-paste)
    if let Some(pos) = trimmed.find("/link ") {
        let after = &trimmed[pos + 6..];
        let code = after.trim();
        if !code.is_empty() {
            let first_word = code.split_whitespace().next().unwrap_or(code);
            // Only match if it looks like a 6-char alphanumeric code
            if first_word.len() == 6 && first_word.chars().all(|c| c.is_ascii_alphanumeric()) {
                return Some(Some(first_word));
            }
        }
    }
    None
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
#[cfg(feature = "dynamodb-backend")]
async fn deduct_credits(
    dynamo: &aws_sdk_dynamodb::Client,
    config_table: &str,
    user_id: &str,
    model: &str,
    input_tokens: u32,
    output_tokens: u32,
) -> i64 {
    let credits = crate::service::auth::calculate_credits(model, input_tokens, output_tokens) as i64;
    if credits == 0 {
        return 0;
    }

    let pk = format!("USER#{}", user_id);

    // Atomic update: decrement credits_remaining, increment credits_used
    let _ = dynamo
        .update_item()
        .table_name(config_table)
        .key("pk", AttributeValue::S(pk))
        .key("sk", AttributeValue::S("PROFILE".to_string()))
        .update_expression("SET credits_remaining = credits_remaining - :c, credits_used = credits_used + :c, updated_at = :now")
        .expression_attribute_values(":c", AttributeValue::N(credits.to_string()))
        .expression_attribute_values(":now", AttributeValue::S(chrono::Utc::now().to_rfc3339()))
        .send()
        .await;

    // Also record usage for analytics
    let usage_pk = format!("USAGE#{}#{}", user_id, chrono::Utc::now().format("%Y-%m-%d"));
    let _ = dynamo
        .put_item()
        .table_name(config_table)
        .item("pk", AttributeValue::S(usage_pk))
        .item("sk", AttributeValue::S(format!("{}#{}", chrono::Utc::now().timestamp_millis(), model)))
        .item("user_id", AttributeValue::S(user_id.to_string()))
        .item("model", AttributeValue::S(model.to_string()))
        .item("input_tokens", AttributeValue::N(input_tokens.to_string()))
        .item("output_tokens", AttributeValue::N(output_tokens.to_string()))
        .item("credits", AttributeValue::N(credits.to_string()))
        .item("timestamp", AttributeValue::S(chrono::Utc::now().to_rfc3339()))
        .send()
        .await;

    credits
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
        description: "General-purpose AI assistant for everyday tasks",
        system_prompt: "あなたはchatweb.ai、高速で賢いAIアシスタントです。\
             日本語で質問されたら日本語で、英語なら英語で答えてください。\
             簡潔で役に立つ回答をしてください。",
        tools_enabled: true,
        icon: "chat",
    },
    AgentProfile {
        id: "researcher",
        name: "Researcher",
        description: "Web research, fact-checking, data gathering",
        system_prompt: "あなたはリサーチ専門のAIエージェントです。\
             手順: 1) web_searchで検索 2) 結果のURLをweb_fetchで取得 3) 取得した情報を元に回答。\
             価格比較は必ずweb_fetchで各サイトの実際の価格を確認し、具体的な金額とURLを含めて回答。\
             ツール結果に実際のデータ（価格、日付等）が含まれていれば、必ずそれを引用して回答。\
             「見つかりません」とは言わず、取得できた情報を最大限活用して回答。\
             情報源のURLを必ず明示。日本語で質問されたら日本語で、英語なら英語で答える。",
        tools_enabled: true,
        icon: "search",
    },
    AgentProfile {
        id: "coder",
        name: "Coder",
        description: "Code writing, debugging, architecture design",
        system_prompt: "あなたはプログラミング専門のAIエージェントです。\
             コードを書く時は必ず言語を明示し、ベストプラクティスに従ってください。\
             バグ修正、コードレビュー、アーキテクチャ設計が得意です。\
             日本語で質問されたら日本語で、英語なら英語で答えてください。",
        tools_enabled: false,
        icon: "code",
    },
    AgentProfile {
        id: "analyst",
        name: "Analyst",
        description: "Data analysis, business insights, financial analysis",
        system_prompt: "あなたはデータ分析専門のAIエージェントです。\
             数値データの分析、ビジネスインサイト、財務分析が得意です。\
             計算ツールを活用し、グラフや表で分かりやすく説明してください。\
             日本語で質問されたら日本語で、英語なら英語で答えてください。",
        tools_enabled: true,
        icon: "chart",
    },
    AgentProfile {
        id: "creative",
        name: "Creative",
        description: "Writing, copywriting, brainstorming, translation",
        system_prompt: "あなたはクリエイティブ専門のAIエージェントです。\
             文章作成、コピーライティング、翻訳、アイデア出しが得意です。\
             ターゲット読者に合わせた表現を心がけ、魅力的なコンテンツを作成してください。\
             日本語で質問されたら日本語で、英語なら英語で答えてください。",
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

    // Research keywords
    if lower.contains("調べ") || lower.contains("検索") || lower.contains("リサーチ")
        || lower.contains("search") || lower.contains("research") || lower.contains("比較")
        || lower.contains("最新") || lower.contains("ニュース") || lower.contains("天気")
        || lower.contains("weather")
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

    // Creative keywords
    if lower.contains("書いて") || lower.contains("翻訳") || lower.contains("メール")
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
        .route("/api/v1/billing/checkout", post(handle_billing_checkout))
        .route("/api/v1/billing/portal", get(handle_billing_portal))
        // Coupon
        .route("/api/v1/coupon/validate", post(handle_coupon_validate))
        // Webhooks
        .route("/webhooks/line", post(handle_line_webhook))
        .route("/webhooks/telegram", post(handle_telegram_webhook))
        .route("/webhooks/stripe", post(handle_stripe_webhook))
        // Pages
        .route("/pricing", get(handle_pricing))
        .route("/welcome", get(handle_welcome))
        .route("/comparison", get(handle_comparison))
        .route("/contact", get(handle_contact))
        // Contact form submission
        .route("/api/v1/contact", post(handle_contact_submit))
        // Status
        .route("/status", get(handle_status))
        // Admin
        .route("/admin", get(handle_admin))
        // OG image
        .route("/og.svg", get(handle_og_svg))
        // Install script
        .route("/install.sh", get(handle_install_sh))
        // Health
        .route("/health", get(handle_health))
        .layer(CompressionLayer::new())
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([
                    http::Method::GET,
                    http::Method::POST,
                    http::Method::PUT,
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
    Json(req): Json<ChatRequest>,
) -> impl IntoResponse {
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

    // Handle /link command
    #[cfg(feature = "dynamodb-backend")]
    if let Some(code_arg) = parse_link_command(&req.message) {
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
            let result = handle_link_command(dynamo, table, &req.session_id, code_arg, &state.sessions).await;
            let response = match result {
                LinkResult::CodeGenerated(msg) | LinkResult::Linked(msg) | LinkResult::Error(msg) => msg,
            };
            return Json(ChatResponse {
                response,
                session_id: req.session_id,
                agent: None,
                tools_used: None,
            });
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
            });
        }
    };

    // Check user credits
    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
            let user = get_or_create_user(dynamo, table, &session_key).await;
            if user.credits_remaining <= 0 {
                return Json(ChatResponse {
                    response: "クレジットが不足しています。プランをアップグレードしてください。\nYou've run out of credits. Please upgrade your plan at /pricing".to_string(),
                    session_id: req.session_id,
                    agent: None,
                    tools_used: None,
                });
            }
        }
    }

    // Check concurrent request limit (10 for free, 1000 for paid)
    let max_concurrent = {
        #[cfg(feature = "dynamodb-backend")]
        {
            let mut limit = 10u32; // free tier default
            if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
                let user = get_or_create_user(dynamo, table, &session_key).await;
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

    // Build conversation with session history — include current date in system prompt
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let system_prompt = format!("{}\n\n今日の日付: {}", agent.system_prompt, today);
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

    let model = &state.config.agents.defaults.model;
    let max_tokens = state.config.agents.defaults.max_tokens;
    let temperature = state.config.agents.defaults.temperature;

    // Get tool definitions for function calling (only if agent supports tools)
    let tools = if agent.tools_enabled {
        state.tool_registry.get_definitions()
    } else {
        vec![]
    };

    let tools_ref = if tools.is_empty() { None } else { Some(&tools[..]) };

    info!("Calling LLM: model={}, tools={}, agent={}", model, tools.len(), agent.id);

    let (response_text, tools_used) = match provider.chat(&messages, tools_ref, model, max_tokens, temperature).await {
        Ok(completion) => {
            info!("LLM response: finish_reason={:?}, tool_calls={}, content_len={}",
                completion.finish_reason, completion.tool_calls.len(),
                completion.content.as_ref().map(|c| c.len()).unwrap_or(0));
            // Deduct credits after successful LLM call
            #[cfg(feature = "dynamodb-backend")]
            {
                if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
                    let credits = deduct_credits(
                        dynamo, table, &session_key, model,
                        completion.usage.prompt_tokens, completion.usage.completion_tokens,
                    ).await;
                    tracing::debug!("Deducted {} credits for user {}", credits, session_key);
                }
            }

            // Handle tool calls: up to 3 rounds (search → fetch → answer)
            let mut current = completion;
            let mut conversation = messages.clone();
            let mut all_tool_results: Vec<(String, String, String)> = Vec::new();

            // Execute tool calls and get final answer
            if current.has_tool_calls() {
                // Limit to max 2 tool calls to stay within API Gateway 29s timeout
                let tool_calls_to_run: Vec<_> = current.tool_calls.iter().take(2).collect();
                if current.tool_calls.len() > 2 {
                    info!("Limiting tool calls from {} to 2", current.tool_calls.len());
                }
                // Execute tool calls in parallel
                let registry = &state.tool_registry;
                let futures: Vec<_> = tool_calls_to_run.iter().map(|tc| {
                    let name = tc.name.clone();
                    let args = tc.arguments.clone();
                    let id = tc.id.clone();
                    async move {
                        info!("Tool call: {} args={:?}", name, args);
                        let result = registry.execute(&name, &args).await;
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

                let tc_json: Vec<serde_json::Value> = current.tool_calls.iter().take(2).map(|tc| {
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

                // Follow-up call WITHOUT tools — force text generation
                match provider.chat(&conversation, None, model, max_tokens, temperature).await {
                    Ok(resp) => {
                        info!("Follow-up: finish={:?}, content_len={}, tool_calls={}",
                            resp.finish_reason,
                            resp.content.as_ref().map(|c| c.len()).unwrap_or(0),
                            resp.tool_calls.len());
                        #[cfg(feature = "dynamodb-backend")]
                        {
                            if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
                                deduct_credits(dynamo, table, &session_key, model,
                                    resp.usage.prompt_tokens, resp.usage.completion_tokens).await;
                            }
                        }
                        current = resp;
                    }
                    Err(e) => {
                        tracing::error!("LLM follow-up error: {}", e);
                        let fallback = all_tool_results.iter()
                            .map(|(_, name, result)| format!("[{}] {}", name, result))
                            .collect::<Vec<_>>().join("\n");
                        current = crate::types::CompletionResponse {
                            content: Some(fallback),
                            tool_calls: vec![],
                            finish_reason: crate::types::FinishReason::Stop,
                            usage: crate::types::TokenUsage::default(),
                        };
                    }
                }
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

    Json(ChatResponse {
        response: response_text,
        session_id: req.session_id,
        agent: Some(agent.id.to_string()),
        tools_used,
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

/// GET /api/v1/usage — Usage info
async fn handle_usage(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    // Extract session/user ID from Authorization header or query
    let user_id = headers.get("x-user-id")
        .and_then(|v| v.to_str().ok())
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
                let welcome = "友だち追加ありがとうございます！\n\nchatweb.ai へようこそ。何でも気軽に聞いてください。\n\n使い方:\n- 何でも質問OK\n- /link でWeb・Telegramと会話を同期\n- WebのセッションIDを送信で自動連携\n\nhttps://chatweb.ai";
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

                    // Handle /link command
                    #[cfg(feature = "dynamodb-backend")]
                    if let Some(code_arg) = parse_link_command(text) {
                        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
                            let result = handle_link_command(dynamo, table, &channel_key, code_arg, &state.sessions).await;
                            let reply = match result {
                                LinkResult::CodeGenerated(msg) | LinkResult::Linked(msg) | LinkResult::Error(msg) => msg,
                            };
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
                                    "あなたはchatweb.ai、高速で賢いAIアシスタントです。\
                                     日本語で質問されたら日本語で、英語なら英語で答えてください。\
                                     簡潔で役に立つ回答をしてください。LINEでのチャットなので短めに。"
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
                                            deduct_credits(dynamo, table, &session_key, model,
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
    body: String,
) -> impl IntoResponse {
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

        let welcome = "Welcome to chatweb.ai!\n\nI'm your AI assistant. Ask me anything!\n\nCommands:\n/link - Sync with Web & LINE\n/start - Show this message\n\nTip: Send your Web session ID to auto-link.\n\nhttps://chatweb.ai";
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

    // Handle /link command
    #[cfg(feature = "dynamodb-backend")]
    if let Some(code_arg) = parse_link_command(text) {
        if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
            let result = handle_link_command(dynamo, table, &channel_key, code_arg, &state.sessions).await;
            let reply = match result {
                LinkResult::CodeGenerated(msg) | LinkResult::Linked(msg) | LinkResult::Error(msg) => msg,
            };
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
                    "あなたはchatweb.ai、高速で賢いAIアシスタントです。\
                     日本語で質問されたら日本語で、英語なら英語で答えてください。\
                     簡潔で役に立つ回答をしてください。Telegramでのチャットなので短めに。"
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
                            deduct_credits(dynamo, table, &session_key, model,
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

                    return Json(serde_json::json!({
                        "valid": true,
                        "code": code,
                        "description": description,
                        "description_ja": description_ja,
                        "stripe_promo_code": stripe_promo,
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

/// GET /api/v1/account/:id — Get user profile (unified billing)
async fn handle_account(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    // Resolve unified session key
    let user_id = {
        #[cfg(feature = "dynamodb-backend")]
        {
            if let (Some(ref dynamo), Some(ref table)) = (&state.dynamo_client, &state.config_table) {
                let resolved = resolve_session_key(dynamo, table, &id).await;
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

    if std::env::var("GEMINI_API_KEY").is_ok() {
        providers.push(serde_json::json!({
            "id": "gemini",
            "name": "Google Gemini",
            "models": ["gemini-2.0-flash", "gemini-pro"],
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
        // Serve API docs for api.chatweb.ai
        axum::response::Html(include_str!("../../../../web/api-docs.html"))
    } else {
        axum::response::Html(include_str!("../../../../web/index.html"))
    }
}

/// GET /pricing — Pricing page
async fn handle_pricing() -> impl IntoResponse {
    axum::response::Html(include_str!("../../../../web/pricing.html"))
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

/// GET /admin — Admin dashboard
async fn handle_admin() -> impl IntoResponse {
    axum::response::Html(include_str!("../../../../web/admin.html"))
}

/// GET /og.svg — OGP image
async fn handle_og_svg() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "image/svg+xml")],
        include_str!("../../../../web/og.svg"),
    )
}

/// GET /install.sh — CLI install script
async fn handle_install_sh() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        include_str!("../../../../web/install.sh"),
    )
}

/// GET /health — Health check
async fn handle_health() -> impl IntoResponse {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: crate::VERSION.to_string(),
    })
}

/// Start the HTTP server on the given address.
pub async fn serve(addr: &str, state: Arc<AppState>) -> anyhow::Result<()> {
    let router = create_router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("HTTP server listening on {}", addr);
    axum::serve(listener, router).await?;
    Ok(())
}
