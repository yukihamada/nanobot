-- nanobot libSQL / SQLite schema
-- Applied on startup via LibSqlBackend::run_migrations()
-- WAL mode is enabled in the connection builder.

PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;

-- ---------------------------------------------------------------------------
-- Users
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS users (
    user_id             TEXT    PRIMARY KEY,
    display_name        TEXT,
    plan                TEXT    NOT NULL DEFAULT 'free',
    credits_remaining   INTEGER NOT NULL DEFAULT 100,
    credits_used        INTEGER NOT NULL DEFAULT 0,
    channels_json       TEXT    NOT NULL DEFAULT '[]',
    stripe_customer_id  TEXT,
    email               TEXT,
    created_at          TEXT    NOT NULL,
    updated_at          TEXT
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_users_email ON users (email) WHERE email IS NOT NULL;

-- ---------------------------------------------------------------------------
-- Email credentials
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS email_credentials (
    user_id         TEXT    NOT NULL,
    email           TEXT    NOT NULL,
    password_hash   TEXT    NOT NULL,
    created_at      TEXT    NOT NULL,
    PRIMARY KEY (email)
);

-- ---------------------------------------------------------------------------
-- Auth tokens
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS auth_tokens (
    token       TEXT    PRIMARY KEY,
    user_id     TEXT    NOT NULL,
    expires_at  TEXT,
    created_at  TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_auth_tokens_user_id ON auth_tokens (user_id);

-- ---------------------------------------------------------------------------
-- API keys
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS api_keys (
    key_id      TEXT    PRIMARY KEY,
    user_id     TEXT    NOT NULL,
    name        TEXT,
    is_active   INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT    NOT NULL,
    last_used_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_api_keys_user_id ON api_keys (user_id);

-- ---------------------------------------------------------------------------
-- Rate limits
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS rate_limits (
    key         TEXT    PRIMARY KEY,
    count       INTEGER NOT NULL DEFAULT 0,
    window_start INTEGER NOT NULL,  -- unix timestamp (seconds)
    expires_at  INTEGER NOT NULL    -- unix timestamp (seconds)
);

-- ---------------------------------------------------------------------------
-- Memory (long-term & daily logs)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS memory (
    user_id     TEXT    NOT NULL,
    kind        TEXT    NOT NULL,   -- 'long_term' | 'daily:YYYY-MM-DD'
    content     TEXT    NOT NULL,
    updated_at  TEXT    NOT NULL,
    PRIMARY KEY (user_id, kind)
);

-- ---------------------------------------------------------------------------
-- Channel mapping (external channel id → user_id)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS channel_map (
    channel_id  TEXT    PRIMARY KEY,
    user_id     TEXT    NOT NULL,
    created_at  TEXT    NOT NULL
);

-- ---------------------------------------------------------------------------
-- Shared conversations
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS shared_conversations (
    hash            TEXT    PRIMARY KEY,
    user_id         TEXT    NOT NULL,
    messages_json   TEXT    NOT NULL,
    created_at      TEXT    NOT NULL
);

-- Reverse index: conv_id → share hash
CREATE TABLE IF NOT EXISTS conv_share_index (
    conv_id     TEXT    PRIMARY KEY,
    hash        TEXT    NOT NULL,
    created_at  TEXT    NOT NULL
);

-- ---------------------------------------------------------------------------
-- Skills marketplace
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS skills (
    id              TEXT    PRIMARY KEY,
    author_user_id  TEXT    NOT NULL,
    name            TEXT    NOT NULL,
    description     TEXT,
    skill_type      TEXT    NOT NULL DEFAULT 'prompt',
    config_json     TEXT    NOT NULL DEFAULT '{}',
    is_public       INTEGER NOT NULL DEFAULT 1,
    created_at      TEXT    NOT NULL,
    updated_at      TEXT
);

CREATE INDEX IF NOT EXISTS idx_skills_author ON skills (author_user_id);
CREATE INDEX IF NOT EXISTS idx_skills_public ON skills (is_public);

CREATE TABLE IF NOT EXISTS installed_skills (
    user_id         TEXT    NOT NULL,
    skill_id        TEXT    NOT NULL,
    webhook_url     TEXT,
    params_json     TEXT,
    installed_at    TEXT    NOT NULL,
    PRIMARY KEY (user_id, skill_id)
);

-- ---------------------------------------------------------------------------
-- Coupons
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS coupons (
    code        TEXT    PRIMARY KEY,
    credits     INTEGER NOT NULL,
    max_uses    INTEGER,
    uses_count  INTEGER NOT NULL DEFAULT 0,
    expires_at  TEXT,
    created_at  TEXT    NOT NULL
);

CREATE TABLE IF NOT EXISTS coupon_redemptions (
    user_id     TEXT    NOT NULL,
    code        TEXT    NOT NULL,
    redeemed_at TEXT    NOT NULL,
    PRIMARY KEY (user_id, code)
);

-- ---------------------------------------------------------------------------
-- Statistics
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS stats_hourly (
    date     TEXT    NOT NULL,
    hour     INTEGER NOT NULL,
    requests INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (date, hour)
);

-- Daily unique users: one row per (date, user_id)
CREATE TABLE IF NOT EXISTS stats_daily_uu (
    date    TEXT    NOT NULL,
    user_id TEXT    NOT NULL,
    PRIMARY KEY (date, user_id)
);

-- ---------------------------------------------------------------------------
-- Audit log
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS audit_logs (
    id          TEXT    PRIMARY KEY,
    date        TEXT    NOT NULL,
    user_id     TEXT,
    action      TEXT    NOT NULL,
    details_json TEXT,
    ip          TEXT,
    created_at  TEXT    NOT NULL,
    expires_at  INTEGER             -- unix timestamp for TTL sweep
);

CREATE INDEX IF NOT EXISTS idx_audit_date ON audit_logs (date);

-- ---------------------------------------------------------------------------
-- A/B testing
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS ab_events (
    id          TEXT    PRIMARY KEY,
    event       TEXT    NOT NULL,
    uid         TEXT    NOT NULL,
    date        TEXT    NOT NULL,
    data_json   TEXT,
    created_at  TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_ab_events_event_date ON ab_events (event, date);

-- ---------------------------------------------------------------------------
-- Provider metrics (TTL via expires_at sweep)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS provider_metrics (
    id              TEXT    PRIMARY KEY,
    provider        TEXT    NOT NULL,
    timestamp_ms    INTEGER NOT NULL,
    latency_ms      INTEGER NOT NULL,
    success         INTEGER NOT NULL,
    created_at      TEXT    NOT NULL,
    expires_at      INTEGER             -- unix timestamp
);

CREATE INDEX IF NOT EXISTS idx_provider_metrics_provider ON provider_metrics (provider, timestamp_ms);

-- ---------------------------------------------------------------------------
-- Config key/value store
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS config_kv (
    pk          TEXT    NOT NULL,
    sk          TEXT    NOT NULL,
    value_json  TEXT    NOT NULL,
    updated_at  TEXT    NOT NULL,
    PRIMARY KEY (pk, sk)
);

-- ---------------------------------------------------------------------------
-- Push subscriptions
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS push_subscriptions (
    user_id     TEXT    NOT NULL,
    endpoint    TEXT    NOT NULL,
    auth        TEXT    NOT NULL,
    p256dh      TEXT    NOT NULL,
    created_at  TEXT    NOT NULL,
    PRIMARY KEY (user_id, endpoint)
);

-- ---------------------------------------------------------------------------
-- Sokora DePIN node registry (persistent across restarts)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS sokora_nodes (
    node_id         TEXT    PRIMARY KEY,
    tunnel_url      TEXT    NOT NULL,
    ram_gb          INTEGER NOT NULL DEFAULT 0,
    models_json     TEXT    NOT NULL DEFAULT '[]',
    version         TEXT    NOT NULL DEFAULT '',
    last_seen       TEXT    NOT NULL,
    -- Provider auth (hashed api_key so we can verify on health check)
    api_key_hash    TEXT,
    -- Reward tracking
    tokens_processed INTEGER NOT NULL DEFAULT 0,
    requests_served  INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sokora_nodes_last_seen ON sokora_nodes (last_seen);
