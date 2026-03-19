/// libSQL / SQLite backend for nanobot.
///
/// Connection modes selected by `DATABASE_URL`:
/// - `libsql://xxx.turso.io`  → Turso cloud (remote)
/// - `/path/to/file.db`       → local SQLite file (Fly.io volume / self-host)
/// - `:memory:`               → in-process SQLite (tests)
use std::sync::Arc;

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use chrono::Utc;
use libsql::{Builder, Connection, Database};
use tracing::{debug, info};
use uuid::Uuid;

use super::backend::{
    AbEvent, ApiKeyRecord, AuditEntry, CouponRecord, DbBackend, HourlyStats,
    InstalledSkill, ProviderMetric, RateLimitResult, SharedConversation, SkillRecord,
    UserProfile,
};

static SCHEMA_SQL: &str = include_str!("schema.sql");

/// libSQL backend. `conn` is a single connection (SQLite serializes writes).
/// For Turso (remote), use the `Database` connection pool.
pub struct LibSqlBackend {
    db: Arc<Database>,
}

impl LibSqlBackend {
    /// Build a backend from a database URL and optional auth token.
    ///
    /// - `libsql://…`  → Turso remote
    /// - anything else → local file (or `:memory:`)
    pub async fn new(url: &str, token: Option<&str>) -> anyhow::Result<Self> {
        let db = if url.starts_with("libsql://") {
            let tok = token.unwrap_or_default().to_string();
            info!("LibSqlBackend: connecting to Turso remote {}", url);
            Builder::new_remote(url.to_string(), tok)
                .build()
                .await
                .context("Failed to build Turso remote connection")?
        } else {
            info!("LibSqlBackend: opening local file {}", url);
            Builder::new_local(url)
                .build()
                .await
                .context("Failed to open local libsql database")?
        };

        Ok(Self { db: Arc::new(db) })
    }

    /// Obtain a connection from the pool/database.
    async fn conn(&self) -> anyhow::Result<Connection> {
        self.db
            .connect()
            .context("Failed to get libsql connection")
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

fn new_id() -> String {
    Uuid::new_v4().to_string()
}

// ---------------------------------------------------------------------------
// DbBackend implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl DbBackend for LibSqlBackend {
    // -----------------------------------------------------------------------
    // Users
    // -----------------------------------------------------------------------

    async fn get_or_create_user(&self, user_id: &str) -> anyhow::Result<UserProfile> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT user_id, display_name, plan, credits_remaining, credits_used, \
                 channels_json, stripe_customer_id, email, created_at, updated_at \
                 FROM users WHERE user_id = ?1",
                libsql::params![user_id],
            )
            .await
            .context("get_or_create_user: query")?;

        if let Some(row) = rows.next().await? {
            return Ok(row_to_user_profile(&row)?);
        }

        // Create with defaults
        let now = now_rfc3339();
        conn.execute(
            "INSERT OR IGNORE INTO users \
             (user_id, plan, credits_remaining, credits_used, channels_json, created_at) \
             VALUES (?1, 'free', 100, 0, '[]', ?2)",
            libsql::params![user_id, now.as_str()],
        )
        .await
        .context("get_or_create_user: insert")?;

        let profile = UserProfile {
            user_id: user_id.to_string(),
            ..Default::default()
        };
        Ok(profile)
    }

    async fn find_user_by_email(&self, email: &str) -> anyhow::Result<Option<UserProfile>> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT user_id, display_name, plan, credits_remaining, credits_used, \
                 channels_json, stripe_customer_id, email, created_at, updated_at \
                 FROM users WHERE email = ?1",
                libsql::params![email],
            )
            .await
            .context("find_user_by_email")?;

        if let Some(row) = rows.next().await? {
            Ok(Some(row_to_user_profile(&row)?))
        } else {
            Ok(None)
        }
    }

    async fn update_user_plan(
        &self,
        user_id: &str,
        plan: &str,
        stripe_customer_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.conn().await?;
        let now = now_rfc3339();
        conn.execute(
            "UPDATE users SET plan = ?1, stripe_customer_id = COALESCE(?2, stripe_customer_id), \
             updated_at = ?3 WHERE user_id = ?4",
            libsql::params![plan, stripe_customer_id, now.as_str(), user_id],
        )
        .await
        .context("update_user_plan")?;
        Ok(())
    }

    async fn update_user_display_name(
        &self,
        user_id: &str,
        display_name: &str,
    ) -> anyhow::Result<()> {
        let conn = self.conn().await?;
        let now = now_rfc3339();
        conn.execute(
            "UPDATE users SET display_name = ?1, updated_at = ?2 WHERE user_id = ?3",
            libsql::params![display_name, now.as_str(), user_id],
        )
        .await
        .context("update_user_display_name")?;
        Ok(())
    }

    async fn add_user_channel(&self, user_id: &str, channel_id: &str) -> anyhow::Result<()> {
        let conn = self.conn().await?;
        // Read → update JSON array → write  (SQLite json_insert is safer)
        conn.execute(
            "UPDATE users SET \
             channels_json = json_insert(channels_json, '$[#]', ?1), \
             updated_at = ?2 \
             WHERE user_id = ?3 AND NOT (channels_json LIKE '%\"' || ?1 || '\"%')",
            libsql::params![channel_id, now_rfc3339().as_str(), user_id],
        )
        .await
        .context("add_user_channel")?;
        Ok(())
    }

    async fn remove_user_channel(&self, user_id: &str, channel_id: &str) -> anyhow::Result<()> {
        let conn = self.conn().await?;
        // Load channels, remove entry, write back
        let mut rows = conn
            .query(
                "SELECT channels_json FROM users WHERE user_id = ?1",
                libsql::params![user_id],
            )
            .await?;

        if let Some(row) = rows.next().await? {
            let json_str: String = row.get(0)?;
            let mut channels: Vec<String> = serde_json::from_str(&json_str).unwrap_or_default();
            channels.retain(|c| c != channel_id);
            let new_json = serde_json::to_string(&channels)?;
            let now = now_rfc3339();
            conn.execute(
                "UPDATE users SET channels_json = ?1, updated_at = ?2 WHERE user_id = ?3",
                libsql::params![new_json.as_str(), now.as_str(), user_id],
            )
            .await
            .context("remove_user_channel: update")?;
        }
        Ok(())
    }

    async fn set_channel_map(&self, channel_id: &str, user_id: &str) -> anyhow::Result<()> {
        let conn = self.conn().await?;
        let now = now_rfc3339();
        conn.execute(
            "INSERT INTO channel_map (channel_id, user_id, created_at) VALUES (?1, ?2, ?3) \
             ON CONFLICT (channel_id) DO UPDATE SET user_id = excluded.user_id",
            libsql::params![channel_id, user_id, now.as_str()],
        )
        .await
        .context("set_channel_map")?;
        Ok(())
    }

    async fn get_channel_map(&self, channel_id: &str) -> anyhow::Result<Option<String>> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT user_id FROM channel_map WHERE channel_id = ?1",
                libsql::params![channel_id],
            )
            .await
            .context("get_channel_map")?;

        if let Some(row) = rows.next().await? {
            let uid: String = row.get(0)?;
            Ok(Some(uid))
        } else {
            Ok(None)
        }
    }

    // -----------------------------------------------------------------------
    // Email credentials
    // -----------------------------------------------------------------------

    async fn set_email_credential(
        &self,
        user_id: &str,
        email: &str,
        password_hash: &str,
    ) -> anyhow::Result<()> {
        let conn = self.conn().await?;
        let now = now_rfc3339();
        conn.execute(
            "INSERT INTO email_credentials (user_id, email, password_hash, created_at) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT (email) DO UPDATE SET password_hash = excluded.password_hash",
            libsql::params![user_id, email, password_hash, now.as_str()],
        )
        .await
        .context("set_email_credential")?;
        // Also update users.email
        conn.execute(
            "UPDATE users SET email = ?1 WHERE user_id = ?2",
            libsql::params![email, user_id],
        )
        .await
        .context("set_email_credential: update users.email")?;
        Ok(())
    }

    async fn get_email_credential(
        &self,
        email: &str,
    ) -> anyhow::Result<Option<(String, String)>> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT user_id, password_hash FROM email_credentials WHERE email = ?1",
                libsql::params![email],
            )
            .await
            .context("get_email_credential")?;

        if let Some(row) = rows.next().await? {
            let uid: String = row.get(0)?;
            let hash: String = row.get(1)?;
            Ok(Some((uid, hash)))
        } else {
            Ok(None)
        }
    }

    // -----------------------------------------------------------------------
    // Auth tokens
    // -----------------------------------------------------------------------

    async fn create_auth_token(
        &self,
        token: &str,
        user_id: &str,
        expires_at: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.conn().await?;
        let now = now_rfc3339();
        conn.execute(
            "INSERT INTO auth_tokens (token, user_id, expires_at, created_at) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT (token) DO NOTHING",
            libsql::params![token, user_id, expires_at, now.as_str()],
        )
        .await
        .context("create_auth_token")?;
        Ok(())
    }

    async fn resolve_auth_token(&self, token: &str) -> anyhow::Result<Option<String>> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT user_id, expires_at FROM auth_tokens WHERE token = ?1",
                libsql::params![token],
            )
            .await
            .context("resolve_auth_token")?;

        if let Some(row) = rows.next().await? {
            let uid: String = row.get(0)?;
            let exp: Option<String> = row.get(1)?;
            // Check expiry
            if let Some(exp_str) = exp {
                if let Ok(exp_dt) = chrono::DateTime::parse_from_rfc3339(&exp_str) {
                    if exp_dt < Utc::now() {
                        return Ok(None);
                    }
                }
            }
            Ok(Some(uid))
        } else {
            Ok(None)
        }
    }

    async fn delete_auth_token(&self, token: &str) -> anyhow::Result<()> {
        let conn = self.conn().await?;
        conn.execute(
            "DELETE FROM auth_tokens WHERE token = ?1",
            libsql::params![token],
        )
        .await
        .context("delete_auth_token")?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // API keys
    // -----------------------------------------------------------------------

    async fn create_api_key(
        &self,
        key_id: &str,
        user_id: &str,
        name: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.conn().await?;
        let now = now_rfc3339();
        conn.execute(
            "INSERT INTO api_keys (key_id, user_id, name, is_active, created_at) \
             VALUES (?1, ?2, ?3, 1, ?4)",
            libsql::params![key_id, user_id, name, now.as_str()],
        )
        .await
        .context("create_api_key")?;
        Ok(())
    }

    async fn lookup_api_key(&self, key_id: &str) -> anyhow::Result<Option<String>> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT user_id FROM api_keys WHERE key_id = ?1 AND is_active = 1",
                libsql::params![key_id],
            )
            .await
            .context("lookup_api_key")?;

        // Update last_used_at asynchronously (fire-and-forget)
        if let Some(row) = rows.next().await? {
            let uid: String = row.get(0)?;
            let now = now_rfc3339();
            let _ = conn
                .execute(
                    "UPDATE api_keys SET last_used_at = ?1 WHERE key_id = ?2",
                    libsql::params![now.as_str(), key_id],
                )
                .await;
            Ok(Some(uid))
        } else {
            Ok(None)
        }
    }

    async fn list_api_keys(&self, user_id: &str) -> anyhow::Result<Vec<ApiKeyRecord>> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT key_id, user_id, name, is_active, created_at, last_used_at \
                 FROM api_keys WHERE user_id = ?1 ORDER BY created_at DESC",
                libsql::params![user_id],
            )
            .await
            .context("list_api_keys")?;

        let mut result = Vec::new();
        while let Some(row) = rows.next().await? {
            let active: i64 = row.get(3)?;
            result.push(ApiKeyRecord {
                key_id: row.get(0)?,
                user_id: row.get(1)?,
                name: row.get(2)?,
                is_active: active != 0,
                created_at: row.get(4)?,
                last_used_at: row.get(5)?,
            });
        }
        Ok(result)
    }

    async fn revoke_api_key(&self, key_id: &str) -> anyhow::Result<()> {
        let conn = self.conn().await?;
        conn.execute(
            "UPDATE api_keys SET is_active = 0 WHERE key_id = ?1",
            libsql::params![key_id],
        )
        .await
        .context("revoke_api_key")?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Credits
    // -----------------------------------------------------------------------

    async fn deduct_credits(
        &self,
        user_id: &str,
        amount: i64,
    ) -> anyhow::Result<(i64, Option<i64>)> {
        let conn = self.conn().await?;

        // Atomic deduction: only succeeds if balance ≥ amount
        let changes = conn
            .execute(
                "UPDATE users SET \
                 credits_remaining = credits_remaining - ?1, \
                 credits_used = credits_used + ?1 \
                 WHERE user_id = ?2 AND credits_remaining >= ?1",
                libsql::params![amount, user_id],
            )
            .await
            .context("deduct_credits: update")?;

        if changes == 0 {
            // Insufficient credits — return 0 deducted, current balance
            let mut rows = conn
                .query(
                    "SELECT credits_remaining FROM users WHERE user_id = ?1",
                    libsql::params![user_id],
                )
                .await?;
            let balance: i64 = if let Some(r) = rows.next().await? {
                r.get(0)?
            } else {
                0
            };
            return Err(anyhow!(
                "insufficient_credits: balance={}, requested={}",
                balance,
                amount
            ));
        }

        // Return new balance
        let mut rows = conn
            .query(
                "SELECT credits_remaining FROM users WHERE user_id = ?1",
                libsql::params![user_id],
            )
            .await?;
        let new_balance: i64 = if let Some(r) = rows.next().await? {
            r.get(0)?
        } else {
            0
        };

        Ok((amount, Some(new_balance)))
    }

    async fn add_credits(&self, user_id: &str, amount: i64) -> anyhow::Result<i64> {
        let conn = self.conn().await?;
        let now = now_rfc3339();
        conn.execute(
            "INSERT INTO users (user_id, credits_remaining, credits_used, plan, channels_json, created_at) \
             VALUES (?1, ?2, 0, 'free', '[]', ?3) \
             ON CONFLICT (user_id) DO UPDATE SET \
             credits_remaining = credits_remaining + ?2, updated_at = ?3",
            libsql::params![user_id, amount, now.as_str()],
        )
        .await
        .context("add_credits")?;

        let mut rows = conn
            .query(
                "SELECT credits_remaining FROM users WHERE user_id = ?1",
                libsql::params![user_id],
            )
            .await?;

        let balance: i64 = if let Some(r) = rows.next().await? {
            r.get(0)?
        } else {
            amount
        };

        Ok(balance)
    }

    // -----------------------------------------------------------------------
    // Rate limits
    // -----------------------------------------------------------------------

    async fn check_rate_limit(
        &self,
        key: &str,
        window_secs: i64,
        max_count: i64,
    ) -> anyhow::Result<RateLimitResult> {
        let conn = self.conn().await?;
        let now_ts = Utc::now().timestamp();
        let window_start = now_ts - window_secs;
        let expires_at = now_ts + window_secs;

        // Expire old windows
        conn.execute(
            "DELETE FROM rate_limits WHERE expires_at < ?1",
            libsql::params![now_ts],
        )
        .await?;

        // Upsert: if window is fresh, increment; else reset
        conn.execute(
            "INSERT INTO rate_limits (key, count, window_start, expires_at) \
             VALUES (?1, 1, ?2, ?3) \
             ON CONFLICT (key) DO UPDATE SET \
               count = CASE \
                 WHEN window_start >= ?2 THEN count + 1 \
                 ELSE 1 \
               END, \
               window_start = CASE \
                 WHEN window_start >= ?2 THEN window_start \
                 ELSE ?2 \
               END, \
               expires_at = ?3",
            libsql::params![key, window_start, expires_at],
        )
        .await
        .context("check_rate_limit: upsert")?;

        let mut rows = conn
            .query(
                "SELECT count FROM rate_limits WHERE key = ?1",
                libsql::params![key],
            )
            .await?;

        let count: i64 = if let Some(r) = rows.next().await? {
            r.get(0)?
        } else {
            1
        };

        Ok(RateLimitResult {
            count,
            exceeded: count > max_count,
        })
    }

    // -----------------------------------------------------------------------
    // Memory
    // -----------------------------------------------------------------------

    async fn get_memory(&self, user_id: &str, kind: &str) -> anyhow::Result<Option<String>> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT content FROM memory WHERE user_id = ?1 AND kind = ?2",
                libsql::params![user_id, kind],
            )
            .await
            .context("get_memory")?;

        if let Some(row) = rows.next().await? {
            let content: String = row.get(0)?;
            Ok(Some(content))
        } else {
            Ok(None)
        }
    }

    async fn set_memory(&self, user_id: &str, kind: &str, content: &str) -> anyhow::Result<()> {
        let conn = self.conn().await?;
        let now = now_rfc3339();
        conn.execute(
            "INSERT INTO memory (user_id, kind, content, updated_at) VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT (user_id, kind) DO UPDATE SET content = excluded.content, updated_at = excluded.updated_at",
            libsql::params![user_id, kind, content, now.as_str()],
        )
        .await
        .context("set_memory")?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Shared conversations
    // -----------------------------------------------------------------------

    async fn create_shared_conversation(
        &self,
        hash: &str,
        user_id: &str,
        messages_json: &str,
    ) -> anyhow::Result<()> {
        let conn = self.conn().await?;
        let now = now_rfc3339();
        conn.execute(
            "INSERT OR IGNORE INTO shared_conversations \
             (hash, user_id, messages_json, created_at) VALUES (?1, ?2, ?3, ?4)",
            libsql::params![hash, user_id, messages_json, now.as_str()],
        )
        .await
        .context("create_shared_conversation")?;
        Ok(())
    }

    async fn get_shared_conversation(
        &self,
        hash: &str,
    ) -> anyhow::Result<Option<SharedConversation>> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT hash, user_id, messages_json, created_at FROM shared_conversations WHERE hash = ?1",
                libsql::params![hash],
            )
            .await
            .context("get_shared_conversation")?;

        if let Some(row) = rows.next().await? {
            Ok(Some(SharedConversation {
                hash: row.get(0)?,
                user_id: row.get(1)?,
                messages_json: row.get(2)?,
                created_at: row.get(3)?,
            }))
        } else {
            Ok(None)
        }
    }

    async fn get_share_hash_for_conv(&self, conv_id: &str) -> anyhow::Result<Option<String>> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT hash FROM conv_share_index WHERE conv_id = ?1",
                libsql::params![conv_id],
            )
            .await
            .context("get_share_hash_for_conv")?;

        if let Some(row) = rows.next().await? {
            let h: String = row.get(0)?;
            Ok(Some(h))
        } else {
            Ok(None)
        }
    }

    async fn set_share_hash_for_conv(&self, conv_id: &str, hash: &str) -> anyhow::Result<()> {
        let conn = self.conn().await?;
        let now = now_rfc3339();
        conn.execute(
            "INSERT OR IGNORE INTO conv_share_index (conv_id, hash, created_at) VALUES (?1, ?2, ?3)",
            libsql::params![conv_id, hash, now.as_str()],
        )
        .await
        .context("set_share_hash_for_conv")?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Skills
    // -----------------------------------------------------------------------

    async fn list_public_skills(&self) -> anyhow::Result<Vec<SkillRecord>> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT id, author_user_id, name, description, skill_type, config_json, \
                 is_public, created_at, updated_at \
                 FROM skills WHERE is_public = 1 ORDER BY created_at DESC",
                (),
            )
            .await
            .context("list_public_skills")?;

        let mut result = Vec::new();
        while let Some(row) = rows.next().await? {
            result.push(row_to_skill(&row)?);
        }
        Ok(result)
    }

    async fn list_skills_by_author(&self, user_id: &str) -> anyhow::Result<Vec<SkillRecord>> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT id, author_user_id, name, description, skill_type, config_json, \
                 is_public, created_at, updated_at \
                 FROM skills WHERE author_user_id = ?1 ORDER BY created_at DESC",
                libsql::params![user_id],
            )
            .await
            .context("list_skills_by_author")?;

        let mut result = Vec::new();
        while let Some(row) = rows.next().await? {
            result.push(row_to_skill(&row)?);
        }
        Ok(result)
    }

    async fn get_skill(&self, skill_id: &str) -> anyhow::Result<Option<SkillRecord>> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT id, author_user_id, name, description, skill_type, config_json, \
                 is_public, created_at, updated_at \
                 FROM skills WHERE id = ?1",
                libsql::params![skill_id],
            )
            .await
            .context("get_skill")?;

        if let Some(row) = rows.next().await? {
            Ok(Some(row_to_skill(&row)?))
        } else {
            Ok(None)
        }
    }

    async fn upsert_skill(&self, skill: &SkillRecord) -> anyhow::Result<()> {
        let conn = self.conn().await?;
        let now = now_rfc3339();
        let is_public: i64 = if skill.is_public { 1 } else { 0 };
        conn.execute(
            "INSERT INTO skills \
             (id, author_user_id, name, description, skill_type, config_json, is_public, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
             ON CONFLICT (id) DO UPDATE SET \
               name = excluded.name, \
               description = excluded.description, \
               config_json = excluded.config_json, \
               is_public = excluded.is_public, \
               updated_at = excluded.updated_at",
            libsql::params![
                skill.id.as_str(),
                skill.author_user_id.as_str(),
                skill.name.as_str(),
                skill.description.as_deref(),
                skill.skill_type.as_str(),
                skill.config_json.as_str(),
                is_public,
                skill.created_at.as_str(),
                now.as_str()
            ],
        )
        .await
        .context("upsert_skill")?;
        Ok(())
    }

    async fn delete_skill(&self, skill_id: &str) -> anyhow::Result<()> {
        let conn = self.conn().await?;
        conn.execute(
            "DELETE FROM skills WHERE id = ?1",
            libsql::params![skill_id],
        )
        .await
        .context("delete_skill")?;
        Ok(())
    }

    async fn install_skill(
        &self,
        user_id: &str,
        skill_id: &str,
        webhook_url: Option<&str>,
        params_json: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.conn().await?;
        let now = now_rfc3339();
        conn.execute(
            "INSERT INTO installed_skills \
             (user_id, skill_id, webhook_url, params_json, installed_at) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT (user_id, skill_id) DO UPDATE SET \
               webhook_url = excluded.webhook_url, \
               params_json = excluded.params_json",
            libsql::params![user_id, skill_id, webhook_url, params_json, now.as_str()],
        )
        .await
        .context("install_skill")?;
        Ok(())
    }

    async fn uninstall_skill(&self, user_id: &str, skill_id: &str) -> anyhow::Result<()> {
        let conn = self.conn().await?;
        conn.execute(
            "DELETE FROM installed_skills WHERE user_id = ?1 AND skill_id = ?2",
            libsql::params![user_id, skill_id],
        )
        .await
        .context("uninstall_skill")?;
        Ok(())
    }

    async fn list_installed_skills(&self, user_id: &str) -> anyhow::Result<Vec<InstalledSkill>> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT user_id, skill_id, webhook_url, params_json, installed_at \
                 FROM installed_skills WHERE user_id = ?1 ORDER BY installed_at DESC",
                libsql::params![user_id],
            )
            .await
            .context("list_installed_skills")?;

        let mut result = Vec::new();
        while let Some(row) = rows.next().await? {
            result.push(InstalledSkill {
                user_id: row.get(0)?,
                skill_id: row.get(1)?,
                webhook_url: row.get(2)?,
                params_json: row.get(3)?,
                installed_at: row.get(4)?,
            });
        }
        Ok(result)
    }

    // -----------------------------------------------------------------------
    // Coupons
    // -----------------------------------------------------------------------

    async fn get_coupon(&self, code: &str) -> anyhow::Result<Option<CouponRecord>> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT code, credits, max_uses, uses_count, expires_at, created_at \
                 FROM coupons WHERE code = ?1",
                libsql::params![code],
            )
            .await
            .context("get_coupon")?;

        if let Some(row) = rows.next().await? {
            Ok(Some(CouponRecord {
                code: row.get(0)?,
                credits: row.get(1)?,
                max_uses: row.get(2)?,
                uses_count: row.get(3)?,
                expires_at: row.get(4)?,
                created_at: row.get(5)?,
            }))
        } else {
            Ok(None)
        }
    }

    async fn redeem_coupon(&self, user_id: &str, code: &str) -> anyhow::Result<i64> {
        let conn = self.conn().await?;

        // Check already redeemed
        if self.has_redeemed_coupon(user_id, code).await? {
            return Err(anyhow!("coupon_already_redeemed"));
        }

        // Get coupon
        let coupon = self
            .get_coupon(code)
            .await?
            .ok_or_else(|| anyhow!("coupon_not_found"))?;

        // Check max uses
        if let Some(max) = coupon.max_uses {
            if coupon.uses_count >= max {
                return Err(anyhow!("coupon_exhausted"));
            }
        }

        // Check expiry
        if let Some(exp) = &coupon.expires_at {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(exp) {
                if dt < Utc::now() {
                    return Err(anyhow!("coupon_expired"));
                }
            }
        }

        // Increment uses
        conn.execute(
            "UPDATE coupons SET uses_count = uses_count + 1 WHERE code = ?1",
            libsql::params![code],
        )
        .await
        .context("redeem_coupon: increment uses")?;

        // Record redemption
        let now = now_rfc3339();
        conn.execute(
            "INSERT INTO coupon_redemptions (user_id, code, redeemed_at) VALUES (?1, ?2, ?3)",
            libsql::params![user_id, code, now.as_str()],
        )
        .await
        .context("redeem_coupon: record")?;

        // Add credits
        self.add_credits(user_id, coupon.credits).await?;

        Ok(coupon.credits)
    }

    async fn has_redeemed_coupon(&self, user_id: &str, code: &str) -> anyhow::Result<bool> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT 1 FROM coupon_redemptions WHERE user_id = ?1 AND code = ?2",
                libsql::params![user_id, code],
            )
            .await
            .context("has_redeemed_coupon")?;

        Ok(rows.next().await?.is_some())
    }

    // -----------------------------------------------------------------------
    // Statistics
    // -----------------------------------------------------------------------

    async fn increment_hourly_stats(&self, date: &str, hour: u32) -> anyhow::Result<()> {
        let conn = self.conn().await?;
        conn.execute(
            "INSERT INTO stats_hourly (date, hour, requests) VALUES (?1, ?2, 1) \
             ON CONFLICT (date, hour) DO UPDATE SET requests = requests + 1",
            libsql::params![date, hour as i64],
        )
        .await
        .context("increment_hourly_stats")?;
        Ok(())
    }

    async fn record_daily_uu(&self, date: &str, user_id: &str) -> anyhow::Result<()> {
        let conn = self.conn().await?;
        conn.execute(
            "INSERT OR IGNORE INTO stats_daily_uu (date, user_id) VALUES (?1, ?2)",
            libsql::params![date, user_id],
        )
        .await
        .context("record_daily_uu")?;
        Ok(())
    }

    async fn get_hourly_stats(
        &self,
        from_date: &str,
        to_date: &str,
    ) -> anyhow::Result<Vec<HourlyStats>> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT date, hour, requests FROM stats_hourly \
                 WHERE date >= ?1 AND date <= ?2 ORDER BY date, hour",
                libsql::params![from_date, to_date],
            )
            .await
            .context("get_hourly_stats")?;

        let mut result = Vec::new();
        while let Some(row) = rows.next().await? {
            let hour_val: i64 = row.get(1)?;
            result.push(HourlyStats {
                date: row.get(0)?,
                hour: hour_val as u32,
                requests: row.get(2)?,
            });
        }
        Ok(result)
    }

    async fn get_daily_uu_counts(
        &self,
        from_date: &str,
        to_date: &str,
    ) -> anyhow::Result<Vec<(String, i64)>> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT date, COUNT(DISTINCT user_id) as uu \
                 FROM stats_daily_uu WHERE date >= ?1 AND date <= ?2 \
                 GROUP BY date ORDER BY date",
                libsql::params![from_date, to_date],
            )
            .await
            .context("get_daily_uu_counts")?;

        let mut result = Vec::new();
        while let Some(row) = rows.next().await? {
            let date: String = row.get(0)?;
            let uu: i64 = row.get(1)?;
            result.push((date, uu));
        }
        Ok(result)
    }

    // -----------------------------------------------------------------------
    // Audit log
    // -----------------------------------------------------------------------

    async fn append_audit(&self, entry: &AuditEntry) -> anyhow::Result<()> {
        let conn = self.conn().await?;
        let expires_at: Option<i64> = entry.ttl_secs.map(|t| Utc::now().timestamp() + t);
        conn.execute(
            "INSERT INTO audit_logs \
             (id, date, user_id, action, details_json, ip, created_at, expires_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            libsql::params![
                entry.id.as_str(),
                entry.date.as_str(),
                entry.user_id.as_deref(),
                entry.action.as_str(),
                entry.details_json.as_deref(),
                entry.ip.as_deref(),
                entry.created_at.as_str(),
                expires_at
            ],
        )
        .await
        .context("append_audit")?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // A/B testing
    // -----------------------------------------------------------------------

    async fn record_ab_event(&self, event: &AbEvent) -> anyhow::Result<()> {
        let conn = self.conn().await?;
        let id = new_id();
        let now = now_rfc3339();
        conn.execute(
            "INSERT OR IGNORE INTO ab_events (id, event, uid, date, data_json, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            libsql::params![
                id.as_str(),
                event.event.as_str(),
                event.uid.as_str(),
                event.date.as_str(),
                event.data_json.as_deref(),
                now.as_str()
            ],
        )
        .await
        .context("record_ab_event")?;
        Ok(())
    }

    async fn get_ab_stats(
        &self,
        event: &str,
        from_date: &str,
        to_date: &str,
    ) -> anyhow::Result<serde_json::Value> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT date, COUNT(*) as count, COUNT(DISTINCT uid) as uu \
                 FROM ab_events WHERE event = ?1 AND date >= ?2 AND date <= ?3 \
                 GROUP BY date ORDER BY date",
                libsql::params![event, from_date, to_date],
            )
            .await
            .context("get_ab_stats")?;

        let mut days = Vec::new();
        while let Some(row) = rows.next().await? {
            let date: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            let uu: i64 = row.get(2)?;
            days.push(serde_json::json!({ "date": date, "count": count, "uu": uu }));
        }
        Ok(serde_json::json!({ "event": event, "days": days }))
    }

    // -----------------------------------------------------------------------
    // Provider metrics
    // -----------------------------------------------------------------------

    async fn record_provider_metric(&self, metric: &ProviderMetric) -> anyhow::Result<()> {
        let conn = self.conn().await?;
        let id = new_id();
        let now = now_rfc3339();
        let expires_at = Utc::now().timestamp() + 300; // 5 min TTL
        let success: i64 = if metric.success { 1 } else { 0 };
        conn.execute(
            "INSERT INTO provider_metrics \
             (id, provider, timestamp_ms, latency_ms, success, created_at, expires_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            libsql::params![
                id.as_str(),
                metric.provider.as_str(),
                metric.timestamp_ms,
                metric.latency_ms as i64,
                success,
                now.as_str(),
                expires_at
            ],
        )
        .await
        .context("record_provider_metric")?;
        Ok(())
    }

    async fn get_provider_metrics(
        &self,
        provider: &str,
        limit: u32,
    ) -> anyhow::Result<Vec<ProviderMetric>> {
        let conn = self.conn().await?;
        let now_ts = Utc::now().timestamp();
        let mut rows = conn
            .query(
                "SELECT provider, timestamp_ms, latency_ms, success \
                 FROM provider_metrics WHERE provider = ?1 AND expires_at > ?2 \
                 ORDER BY timestamp_ms DESC LIMIT ?3",
                libsql::params![provider, now_ts, limit as i64],
            )
            .await
            .context("get_provider_metrics")?;

        let mut result = Vec::new();
        while let Some(row) = rows.next().await? {
            let success_val: i64 = row.get(3)?;
            result.push(ProviderMetric {
                provider: row.get(0)?,
                timestamp_ms: row.get(1)?,
                latency_ms: {
                    let v: i64 = row.get(2)?;
                    v as u64
                },
                success: success_val != 0,
            });
        }
        Ok(result)
    }

    // -----------------------------------------------------------------------
    // Config
    // -----------------------------------------------------------------------

    async fn get_config(&self, pk: &str, sk: &str) -> anyhow::Result<Option<serde_json::Value>> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT value_json FROM config_kv WHERE pk = ?1 AND sk = ?2",
                libsql::params![pk, sk],
            )
            .await
            .context("get_config")?;

        if let Some(row) = rows.next().await? {
            let json_str: String = row.get(0)?;
            let value = serde_json::from_str(&json_str).context("get_config: parse json")?;
            Ok(Some(value))
        } else {
            Ok(None)
        }
    }

    async fn set_config(
        &self,
        pk: &str,
        sk: &str,
        value: &serde_json::Value,
    ) -> anyhow::Result<()> {
        let conn = self.conn().await?;
        let now = now_rfc3339();
        let json_str = serde_json::to_string(value)?;
        conn.execute(
            "INSERT INTO config_kv (pk, sk, value_json, updated_at) VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT (pk, sk) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
            libsql::params![pk, sk, json_str.as_str(), now.as_str()],
        )
        .await
        .context("set_config")?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Push subscriptions
    // -----------------------------------------------------------------------

    async fn upsert_push_subscription(
        &self,
        user_id: &str,
        endpoint: &str,
        auth: &str,
        p256dh: &str,
    ) -> anyhow::Result<()> {
        let conn = self.conn().await?;
        let now = now_rfc3339();
        conn.execute(
            "INSERT INTO push_subscriptions (user_id, endpoint, auth, p256dh, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT (user_id, endpoint) DO UPDATE SET auth = excluded.auth, p256dh = excluded.p256dh",
            libsql::params![user_id, endpoint, auth, p256dh, now.as_str()],
        )
        .await
        .context("upsert_push_subscription")?;
        Ok(())
    }

    async fn get_push_subscriptions(
        &self,
        user_id: &str,
    ) -> anyhow::Result<Vec<(String, String, String)>> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT endpoint, auth, p256dh FROM push_subscriptions WHERE user_id = ?1",
                libsql::params![user_id],
            )
            .await
            .context("get_push_subscriptions")?;

        let mut result = Vec::new();
        while let Some(row) = rows.next().await? {
            result.push((row.get(0)?, row.get(1)?, row.get(2)?));
        }
        Ok(result)
    }

    // -----------------------------------------------------------------------
    // Migrations
    // -----------------------------------------------------------------------

    async fn run_migrations(&self) -> anyhow::Result<()> {
        info!("LibSqlBackend: running schema migrations");
        let conn = self.conn().await?;

        // Execute each statement separated by semicolons.
        // Strip leading SQL comment lines (--) from each segment before
        // deciding whether it is empty / pure-comment.
        for raw in SCHEMA_SQL.split(';') {
            // Remove leading comment lines so that "-- ...\nCREATE TABLE"
            // is not accidentally filtered out by a starts_with("--") check.
            let stmt: String = raw
                .lines()
                .filter(|line| {
                    let t = line.trim();
                    !t.is_empty() && !t.starts_with("--")
                })
                .collect::<Vec<_>>()
                .join("\n");
            let stmt = stmt.trim();
            if stmt.is_empty() {
                continue;
            }
            debug!("LibSqlBackend migration: {}", &stmt[..stmt.len().min(60)]);
            // PRAGMA statements return rows — use query() to avoid
            // "Execute returned rows" error from conn.execute().
            let upper = stmt.to_uppercase();
            if upper.starts_with("PRAGMA") {
                conn.query(stmt, ())
                    .await
                    .with_context(|| format!("Migration failed for statement: {}", &stmt[..stmt.len().min(80)]))?;
            } else {
                conn.execute(stmt, ())
                    .await
                    .with_context(|| format!("Migration failed for statement: {}", &stmt[..stmt.len().min(80)]))?;
            }
        }

        info!("LibSqlBackend: migrations complete");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Row helpers
// ---------------------------------------------------------------------------

fn row_to_user_profile(row: &libsql::Row) -> anyhow::Result<UserProfile> {
    let channels_json: String = row.get(5).unwrap_or_else(|_| "[]".to_string());
    let channels: Vec<String> = serde_json::from_str(&channels_json).unwrap_or_default();
    Ok(UserProfile {
        user_id: row.get(0)?,
        display_name: row.get(1)?,
        plan: row.get::<String>(2).unwrap_or_else(|_| "free".to_string()),
        credits_remaining: row.get::<i64>(3).unwrap_or(100),
        credits_used: row.get::<i64>(4).unwrap_or(0),
        channels,
        stripe_customer_id: row.get(6)?,
        email: row.get(7)?,
        created_at: row.get::<String>(8).unwrap_or_else(|_| now_rfc3339()),
        updated_at: row.get(9)?,
    })
}

fn row_to_skill(row: &libsql::Row) -> anyhow::Result<SkillRecord> {
    let public_val: i64 = row.get(6).unwrap_or(1);
    Ok(SkillRecord {
        id: row.get(0)?,
        author_user_id: row.get(1)?,
        name: row.get(2)?,
        description: row.get(3)?,
        skill_type: row.get::<String>(4).unwrap_or_else(|_| "prompt".to_string()),
        config_json: row.get::<String>(5).unwrap_or_else(|_| "{}".to_string()),
        is_public: public_val != 0,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}
