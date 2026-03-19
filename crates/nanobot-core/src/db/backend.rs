/// DbBackend trait: database abstraction for nanobot.
///
/// Supports both DynamoDB (existing) and libSQL/SQLite (new) backends.
/// All methods are async and return `anyhow::Result`.
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Full user profile stored in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub user_id: String,
    pub display_name: Option<String>,
    pub plan: String,
    pub credits_remaining: i64,
    pub credits_used: i64,
    /// JSON-encoded list of linked channel IDs
    pub channels: Vec<String>,
    pub stripe_customer_id: Option<String>,
    pub email: Option<String>,
    pub created_at: String,
    pub updated_at: Option<String>,
}

impl Default for UserProfile {
    fn default() -> Self {
        Self {
            user_id: String::new(),
            display_name: None,
            plan: "free".to_string(),
            credits_remaining: 100,
            credits_used: 0,
            channels: vec![],
            stripe_customer_id: None,
            email: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: None,
        }
    }
}

/// A resolved authentication token.
#[derive(Debug, Clone)]
pub struct AuthToken {
    pub token: String,
    pub user_id: String,
    pub expires_at: Option<String>,
}

/// An API key record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyRecord {
    pub key_id: String,
    pub user_id: String,
    pub name: Option<String>,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub is_active: bool,
}

/// A rate-limit counter result.
#[derive(Debug, Clone)]
pub struct RateLimitResult {
    /// Current count within this window.
    pub count: i64,
    /// Whether the limit has been exceeded.
    pub exceeded: bool,
}

/// A skill record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRecord {
    pub id: String,
    pub author_user_id: String,
    pub name: String,
    pub description: Option<String>,
    pub skill_type: String,
    pub config_json: String,
    pub is_public: bool,
    pub created_at: String,
    pub updated_at: Option<String>,
}

/// An installed skill (user → skill mapping).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledSkill {
    pub user_id: String,
    pub skill_id: String,
    pub webhook_url: Option<String>,
    pub params_json: Option<String>,
    pub installed_at: String,
}

/// A coupon record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CouponRecord {
    pub code: String,
    pub credits: i64,
    pub max_uses: Option<i64>,
    pub uses_count: i64,
    pub expires_at: Option<String>,
    pub created_at: String,
}

/// A shared conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedConversation {
    pub hash: String,
    pub user_id: String,
    pub messages_json: String,
    pub created_at: String,
}

/// Hourly stats record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HourlyStats {
    pub date: String,
    pub hour: u32,
    pub requests: i64,
}

/// Provider metric record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderMetric {
    pub provider: String,
    pub timestamp_ms: i64,
    pub latency_ms: u64,
    pub success: bool,
}

/// Audit log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: String,
    pub date: String,
    pub user_id: Option<String>,
    pub action: String,
    pub details_json: Option<String>,
    pub ip: Option<String>,
    pub created_at: String,
    pub ttl_secs: Option<i64>,
}

/// A/B test event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbEvent {
    pub event: String,
    pub uid: String,
    pub date: String,
    pub data_json: Option<String>,
}

/// Database backend trait.
///
/// Implementations:
/// - `DynamoDbBackend` (crate feature `dynamodb-backend`)
/// - `LibSqlBackend` (crate feature `libsql-backend`)
#[async_trait]
pub trait DbBackend: Send + Sync {
    // -----------------------------------------------------------------------
    // Users
    // -----------------------------------------------------------------------

    /// Get a user by ID, creating with default free plan if not found.
    async fn get_or_create_user(&self, user_id: &str) -> anyhow::Result<UserProfile>;

    /// Look up a user by their email address.
    async fn find_user_by_email(&self, email: &str) -> anyhow::Result<Option<UserProfile>>;

    /// Update a user's plan and/or Stripe customer ID.
    async fn update_user_plan(
        &self,
        user_id: &str,
        plan: &str,
        stripe_customer_id: Option<&str>,
    ) -> anyhow::Result<()>;

    /// Update user's display name.
    async fn update_user_display_name(
        &self,
        user_id: &str,
        display_name: &str,
    ) -> anyhow::Result<()>;

    /// Add a channel to a user's channel list.
    async fn add_user_channel(&self, user_id: &str, channel_id: &str) -> anyhow::Result<()>;

    /// Remove a channel from a user's channel list.
    async fn remove_user_channel(&self, user_id: &str, channel_id: &str) -> anyhow::Result<()>;

    /// Set the mapping from an external channel ID to a user_id.
    async fn set_channel_map(&self, channel_id: &str, user_id: &str) -> anyhow::Result<()>;

    /// Resolve an external channel ID to a user_id.
    async fn get_channel_map(&self, channel_id: &str) -> anyhow::Result<Option<String>>;

    // -----------------------------------------------------------------------
    // Email credentials
    // -----------------------------------------------------------------------

    /// Store hashed password for a user's email.
    async fn set_email_credential(
        &self,
        user_id: &str,
        email: &str,
        password_hash: &str,
    ) -> anyhow::Result<()>;

    /// Retrieve password hash for an email.
    async fn get_email_credential(
        &self,
        email: &str,
    ) -> anyhow::Result<Option<(String, String)>>; // (user_id, password_hash)

    // -----------------------------------------------------------------------
    // Auth tokens
    // -----------------------------------------------------------------------

    /// Store an auth token mapping to a user_id with optional expiry.
    async fn create_auth_token(
        &self,
        token: &str,
        user_id: &str,
        expires_at: Option<&str>,
    ) -> anyhow::Result<()>;

    /// Resolve an auth token to a user_id (returns None if expired or missing).
    async fn resolve_auth_token(&self, token: &str) -> anyhow::Result<Option<String>>;

    /// Delete an auth token (logout).
    async fn delete_auth_token(&self, token: &str) -> anyhow::Result<()>;

    // -----------------------------------------------------------------------
    // API keys
    // -----------------------------------------------------------------------

    /// Create a new API key record.
    async fn create_api_key(
        &self,
        key_id: &str,
        user_id: &str,
        name: Option<&str>,
    ) -> anyhow::Result<()>;

    /// Look up the user_id for an API key.
    async fn lookup_api_key(&self, key_id: &str) -> anyhow::Result<Option<String>>;

    /// List all API keys for a user.
    async fn list_api_keys(&self, user_id: &str) -> anyhow::Result<Vec<ApiKeyRecord>>;

    /// Revoke (deactivate) an API key.
    async fn revoke_api_key(&self, key_id: &str) -> anyhow::Result<()>;

    // -----------------------------------------------------------------------
    // Credits (atomic operations)
    // -----------------------------------------------------------------------

    /// Atomically deduct credits from a user.
    ///
    /// Returns `(credits_deducted, credits_remaining)`.
    /// Returns an error if the balance would go negative.
    async fn deduct_credits(
        &self,
        user_id: &str,
        amount: i64,
    ) -> anyhow::Result<(i64, Option<i64>)>;

    /// Add credits to a user (e.g., purchase, coupon, apology grant).
    ///
    /// Returns the new balance.
    async fn add_credits(&self, user_id: &str, amount: i64) -> anyhow::Result<i64>;

    // -----------------------------------------------------------------------
    // Rate limits
    // -----------------------------------------------------------------------

    /// Increment a rate-limit counter and check if the limit is exceeded.
    ///
    /// `key` identifies the counter (e.g., `"login:{ip}"`).
    /// `window_secs` is the rolling window duration.
    /// `max_count` is the maximum allowed within the window.
    async fn check_rate_limit(
        &self,
        key: &str,
        window_secs: i64,
        max_count: i64,
    ) -> anyhow::Result<RateLimitResult>;

    // -----------------------------------------------------------------------
    // Memory (long-term & daily)
    // -----------------------------------------------------------------------

    /// Read a memory entry. `kind` is "long_term" or "daily:{YYYY-MM-DD}".
    async fn get_memory(&self, user_id: &str, kind: &str) -> anyhow::Result<Option<String>>;

    /// Write a memory entry (upsert).
    async fn set_memory(&self, user_id: &str, kind: &str, content: &str) -> anyhow::Result<()>;

    // -----------------------------------------------------------------------
    // Shared conversations
    // -----------------------------------------------------------------------

    /// Store a shared conversation, returning its hash.
    async fn create_shared_conversation(
        &self,
        hash: &str,
        user_id: &str,
        messages_json: &str,
    ) -> anyhow::Result<()>;

    /// Load a shared conversation by hash.
    async fn get_shared_conversation(
        &self,
        hash: &str,
    ) -> anyhow::Result<Option<SharedConversation>>;

    /// Look up the share hash for a conversation ID (reverse index).
    async fn get_share_hash_for_conv(&self, conv_id: &str) -> anyhow::Result<Option<String>>;

    /// Store the conv_id → share_hash reverse mapping.
    async fn set_share_hash_for_conv(&self, conv_id: &str, hash: &str) -> anyhow::Result<()>;

    // -----------------------------------------------------------------------
    // Skills marketplace
    // -----------------------------------------------------------------------

    /// List all public skills.
    async fn list_public_skills(&self) -> anyhow::Result<Vec<SkillRecord>>;

    /// List skills authored by a user.
    async fn list_skills_by_author(&self, user_id: &str) -> anyhow::Result<Vec<SkillRecord>>;

    /// Get a skill by ID.
    async fn get_skill(&self, skill_id: &str) -> anyhow::Result<Option<SkillRecord>>;

    /// Create or update a skill.
    async fn upsert_skill(&self, skill: &SkillRecord) -> anyhow::Result<()>;

    /// Delete a skill (only by its author).
    async fn delete_skill(&self, skill_id: &str) -> anyhow::Result<()>;

    /// Install a skill for a user.
    async fn install_skill(
        &self,
        user_id: &str,
        skill_id: &str,
        webhook_url: Option<&str>,
        params_json: Option<&str>,
    ) -> anyhow::Result<()>;

    /// Uninstall a skill for a user.
    async fn uninstall_skill(&self, user_id: &str, skill_id: &str) -> anyhow::Result<()>;

    /// List installed skills for a user.
    async fn list_installed_skills(&self, user_id: &str) -> anyhow::Result<Vec<InstalledSkill>>;

    // -----------------------------------------------------------------------
    // Coupons
    // -----------------------------------------------------------------------

    /// Look up a coupon by code.
    async fn get_coupon(&self, code: &str) -> anyhow::Result<Option<CouponRecord>>;

    /// Redeem a coupon for a user (atomic: increments uses_count and marks as redeemed).
    /// Returns the credits granted.
    async fn redeem_coupon(&self, user_id: &str, code: &str) -> anyhow::Result<i64>;

    /// Check if a user has already redeemed a coupon.
    async fn has_redeemed_coupon(&self, user_id: &str, code: &str) -> anyhow::Result<bool>;

    // -----------------------------------------------------------------------
    // Statistics
    // -----------------------------------------------------------------------

    /// Increment hourly request counter (upsert).
    async fn increment_hourly_stats(&self, date: &str, hour: u32) -> anyhow::Result<()>;

    /// Record a unique user for daily stats.
    async fn record_daily_uu(&self, date: &str, user_id: &str) -> anyhow::Result<()>;

    /// Get hourly stats for a date range.
    async fn get_hourly_stats(&self, from_date: &str, to_date: &str)
        -> anyhow::Result<Vec<HourlyStats>>;

    /// Get daily unique user counts.
    async fn get_daily_uu_counts(
        &self,
        from_date: &str,
        to_date: &str,
    ) -> anyhow::Result<Vec<(String, i64)>>;

    // -----------------------------------------------------------------------
    // Audit log
    // -----------------------------------------------------------------------

    /// Append an audit log entry.
    async fn append_audit(&self, entry: &AuditEntry) -> anyhow::Result<()>;

    // -----------------------------------------------------------------------
    // A/B testing
    // -----------------------------------------------------------------------

    /// Record an A/B test event.
    async fn record_ab_event(&self, event: &AbEvent) -> anyhow::Result<()>;

    /// Get aggregated A/B stats for an event over a date range.
    async fn get_ab_stats(
        &self,
        event: &str,
        from_date: &str,
        to_date: &str,
    ) -> anyhow::Result<serde_json::Value>;

    // -----------------------------------------------------------------------
    // Provider metrics
    // -----------------------------------------------------------------------

    /// Record a provider latency/success metric.
    async fn record_provider_metric(&self, metric: &ProviderMetric) -> anyhow::Result<()>;

    /// Get recent provider health metrics.
    async fn get_provider_metrics(
        &self,
        provider: &str,
        limit: u32,
    ) -> anyhow::Result<Vec<ProviderMetric>>;

    // -----------------------------------------------------------------------
    // Config (key/value store)
    // -----------------------------------------------------------------------

    /// Get a config value by pk + sk.
    async fn get_config(&self, pk: &str, sk: &str) -> anyhow::Result<Option<serde_json::Value>>;

    /// Set a config value.
    async fn set_config(
        &self,
        pk: &str,
        sk: &str,
        value: &serde_json::Value,
    ) -> anyhow::Result<()>;

    // -----------------------------------------------------------------------
    // Push subscriptions
    // -----------------------------------------------------------------------

    /// Upsert a Web Push subscription for a user.
    async fn upsert_push_subscription(
        &self,
        user_id: &str,
        endpoint: &str,
        auth: &str,
        p256dh: &str,
    ) -> anyhow::Result<()>;

    /// Get all push subscriptions for a user.
    async fn get_push_subscriptions(
        &self,
        user_id: &str,
    ) -> anyhow::Result<Vec<(String, String, String)>>; // (endpoint, auth, p256dh)

    // -----------------------------------------------------------------------
    // Migrations (run once on startup)
    // -----------------------------------------------------------------------

    /// Run schema migrations. No-op for DynamoDB; applies SQL for libSQL.
    async fn run_migrations(&self) -> anyhow::Result<()>;
}
