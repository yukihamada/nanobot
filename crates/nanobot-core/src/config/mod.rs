pub mod provider;

#[cfg(feature = "dynamodb-backend")]
pub mod dynamo_provider;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::error::ConfigError;

/// Root configuration for nanobot.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[derive(Default)]
pub struct Config {
    pub agents: AgentsConfig,
    pub channels: ChannelsConfig,
    pub providers: ProvidersConfig,
    pub gateway: GatewayConfig,
    pub tools: ToolsConfig,
}


impl Config {
    /// Get expanded workspace path.
    pub fn workspace_path(&self) -> PathBuf {
        let path = &self.agents.defaults.workspace;
        if path.starts_with("~/") || path.starts_with("~\\") {
            if let Some(home) = dirs::home_dir() {
                return home.join(&path[2..]);
            }
        }
        PathBuf::from(path)
    }

    /// Match a provider based on model name.
    fn match_provider(&self, model: Option<&str>) -> Option<&ProviderConfig> {
        let model = model
            .unwrap_or(&self.agents.defaults.model)
            .to_lowercase();

        let providers: &[(&[&str], &ProviderConfig)] = &[
            (&["openrouter"], &self.providers.openrouter),
            (&["deepseek"], &self.providers.deepseek),
            (&["anthropic", "claude"], &self.providers.anthropic),
            (&["openai", "gpt"], &self.providers.openai),
            (&["gemini"], &self.providers.gemini),
            (&["zhipu", "glm", "zai"], &self.providers.zhipu),
            (&["groq"], &self.providers.groq),
            (&["moonshot", "kimi"], &self.providers.moonshot),
            (&["vllm"], &self.providers.vllm),
        ];

        for (keywords, provider) in providers {
            for keyword in *keywords {
                if model.contains(keyword) && !provider.api_key.is_empty() {
                    return Some(provider);
                }
            }
        }
        None
    }

    /// Get API key for the given model (or default model).
    /// Falls back to first available key.
    pub fn get_api_key(&self, model: Option<&str>) -> Option<&str> {
        if let Some(p) = self.match_provider(model) {
            return Some(&p.api_key);
        }
        // Fallback: return first available key
        let providers = [
            &self.providers.openrouter,
            &self.providers.deepseek,
            &self.providers.anthropic,
            &self.providers.openai,
            &self.providers.gemini,
            &self.providers.zhipu,
            &self.providers.moonshot,
            &self.providers.vllm,
            &self.providers.groq,
        ];
        for p in providers {
            if !p.api_key.is_empty() {
                return Some(&p.api_key);
            }
        }
        None
    }

    /// Get API base URL based on model name.
    pub fn get_api_base(&self, model: Option<&str>) -> Option<&str> {
        let model = model
            .unwrap_or(&self.agents.defaults.model)
            .to_lowercase();

        if model.contains("openrouter") {
            return Some(
                self.providers
                    .openrouter
                    .api_base
                    .as_deref()
                    .unwrap_or("https://openrouter.ai/api/v1"),
            );
        }
        if ["zhipu", "glm", "zai"].iter().any(|k| model.contains(k)) {
            return self.providers.zhipu.api_base.as_deref();
        }
        if model.contains("vllm") {
            return self.providers.vllm.api_base.as_deref();
        }
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[derive(Default)]
pub struct AgentsConfig {
    pub defaults: AgentDefaults,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentDefaults {
    pub workspace: String,
    pub model: String,
    pub max_tokens: u32,
    pub temperature: f64,
    pub max_tool_iterations: u32,
}

impl Default for AgentDefaults {
    fn default() -> Self {
        Self {
            workspace: "~/.nanobot/workspace".to_string(),
            model: "anthropic/claude-opus-4-5".to_string(),
            max_tokens: 8192,
            temperature: 0.7,
            max_tool_iterations: 20,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[derive(Default)]
pub struct ChannelsConfig {
    pub whatsapp: WhatsAppConfig,
    pub telegram: TelegramConfig,
    pub discord: DiscordConfig,
    pub feishu: FeishuConfig,
    pub line: LineConfig,
    pub slack: SlackConfig,
    pub signal: SignalConfig,
    pub imessage: IMessageConfig,
    pub teams: TeamsConfig,
    pub google_chat: GoogleChatConfig,
    pub matrix: MatrixConfig,
    pub zalo: ZaloConfig,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[derive(Default)]
pub struct LineConfig {
    pub enabled: bool,
    pub channel_secret: String,
    pub channel_access_token: String,
    pub allow_from: Vec<String>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WhatsAppConfig {
    pub enabled: bool,
    pub bridge_url: String,
    pub allow_from: Vec<String>,
}

impl Default for WhatsAppConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bridge_url: "ws://localhost:3001".to_string(),
            allow_from: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[derive(Default)]
pub struct TelegramConfig {
    pub enabled: bool,
    pub token: String,
    pub allow_from: Vec<String>,
    pub proxy: Option<String>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DiscordConfig {
    pub enabled: bool,
    pub token: String,
    pub allow_from: Vec<String>,
    pub gateway_url: String,
    pub intents: u32,
}

impl Default for DiscordConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            token: String::new(),
            allow_from: Vec::new(),
            gateway_url: "wss://gateway.discord.gg/?v=10&encoding=json".to_string(),
            intents: 37377,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[derive(Default)]
pub struct FeishuConfig {
    pub enabled: bool,
    pub app_id: String,
    pub app_secret: String,
    pub encrypt_key: String,
    pub verification_token: String,
    pub allow_from: Vec<String>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[derive(Default)]
pub struct SlackConfig {
    pub enabled: bool,
    pub app_token: String,
    pub bot_token: String,
    pub allow_from: Vec<String>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SignalConfig {
    pub enabled: bool,
    pub endpoint: String,
    pub phone_number: String,
    pub allow_from: Vec<String>,
}

impl Default for SignalConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: "http://localhost:8080".to_string(),
            phone_number: String::new(),
            allow_from: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct IMessageConfig {
    pub enabled: bool,
    pub bridge_url: String,
    pub allow_from: Vec<String>,
}

impl Default for IMessageConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bridge_url: "http://localhost:1234".to_string(),
            allow_from: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[derive(Default)]
pub struct TeamsConfig {
    pub enabled: bool,
    pub app_id: String,
    pub app_password: String,
    pub allow_from: Vec<String>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[derive(Default)]
pub struct GoogleChatConfig {
    pub enabled: bool,
    pub service_account_key: String,
    pub webhook_token: String,
    pub allow_from: Vec<String>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[derive(Default)]
pub struct MatrixConfig {
    pub enabled: bool,
    pub homeserver: String,
    pub user_id: String,
    pub access_token: String,
    pub allow_from: Vec<String>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[derive(Default)]
pub struct ZaloConfig {
    pub enabled: bool,
    pub bot_token: String,
    pub secret_token: String,
    pub allow_from: Vec<String>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[derive(Default)]
pub struct ProvidersConfig {
    pub anthropic: ProviderConfig,
    pub openai: ProviderConfig,
    pub openrouter: ProviderConfig,
    pub deepseek: ProviderConfig,
    pub groq: ProviderConfig,
    pub zhipu: ProviderConfig,
    pub vllm: ProviderConfig,
    pub gemini: ProviderConfig,
    pub moonshot: ProviderConfig,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[derive(Default)]
pub struct ProviderConfig {
    pub api_key: String,
    pub api_base: Option<String>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GatewayConfig {
    pub host: String,
    pub port: u16,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 18790,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[derive(Default)]
pub struct ToolsConfig {
    pub web: WebToolsConfig,
    #[serde(rename = "exec")]
    pub exec_config: ExecToolConfig,
    pub restrict_to_workspace: bool,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[derive(Default)]
pub struct WebToolsConfig {
    pub search: WebSearchConfig,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WebSearchConfig {
    pub api_key: String,
    pub max_results: u32,
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            max_results: 5,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExecToolConfig {
    pub timeout: u64,
}

impl Default for ExecToolConfig {
    fn default() -> Self {
        Self { timeout: 60 }
    }
}

// ====== Config loading/saving ======

/// Load configuration from environment variables.
///
/// Priority:
/// 1. `NANOBOT_CONFIG` env var — full JSON config
/// 2. Individual env vars (merged on top of defaults)
/// 3. File fallback (`~/.nanobot/config.json`)
pub fn load_config_from_env() -> Config {
    // 1. Full JSON from NANOBOT_CONFIG
    if let Ok(json) = std::env::var("NANOBOT_CONFIG") {
        match serde_json::from_str::<Config>(&json) {
            Ok(config) => return config,
            Err(e) => {
                tracing::warn!("Failed to parse NANOBOT_CONFIG: {}", e);
            }
        }
    }

    // 2. Start with file fallback, then overlay individual env vars
    let mut cfg = load_config(None);

    // Provider keys
    if let Ok(v) = std::env::var("ANTHROPIC_API_KEY") {
        cfg.providers.anthropic.api_key = v;
    }
    if let Ok(v) = std::env::var("OPENAI_API_KEY") {
        cfg.providers.openai.api_key = v;
    }
    if let Ok(v) = std::env::var("OPENROUTER_API_KEY") {
        cfg.providers.openrouter.api_key = v;
    }
    if let Ok(v) = std::env::var("DEEPSEEK_API_KEY") {
        cfg.providers.deepseek.api_key = v;
    }

    // Telegram
    if let Ok(v) = std::env::var("TELEGRAM_BOT_TOKEN") {
        cfg.channels.telegram.token = v;
        cfg.channels.telegram.enabled = true;
    }
    if let Ok(v) = std::env::var("TELEGRAM_ALLOW_FROM") {
        cfg.channels.telegram.allow_from = v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    }

    // LINE
    if let Ok(v) = std::env::var("LINE_CHANNEL_SECRET") {
        cfg.channels.line.channel_secret = v;
    }
    if let Ok(v) = std::env::var("LINE_CHANNEL_ACCESS_TOKEN") {
        cfg.channels.line.channel_access_token = v;
        cfg.channels.line.enabled = true;
    }
    if let Ok(v) = std::env::var("LINE_ALLOW_FROM") {
        cfg.channels.line.allow_from = v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    }

    // Discord
    if let Ok(v) = std::env::var("DISCORD_BOT_TOKEN") {
        cfg.channels.discord.token = v;
        cfg.channels.discord.enabled = true;
    }

    // Slack
    if let Ok(v) = std::env::var("SLACK_APP_TOKEN") {
        cfg.channels.slack.app_token = v;
    }
    if let Ok(v) = std::env::var("SLACK_BOT_TOKEN") {
        cfg.channels.slack.bot_token = v;
        cfg.channels.slack.enabled = true;
    }

    // Signal
    if let Ok(v) = std::env::var("SIGNAL_ENDPOINT") {
        cfg.channels.signal.endpoint = v;
    }
    if let Ok(v) = std::env::var("SIGNAL_PHONE") {
        cfg.channels.signal.phone_number = v;
        cfg.channels.signal.enabled = true;
    }

    // iMessage
    if let Ok(v) = std::env::var("IMESSAGE_BRIDGE_URL") {
        cfg.channels.imessage.bridge_url = v;
        cfg.channels.imessage.enabled = true;
    }

    // MS Teams
    if let Ok(v) = std::env::var("TEAMS_APP_ID") {
        cfg.channels.teams.app_id = v;
    }
    if let Ok(v) = std::env::var("TEAMS_APP_PASSWORD") {
        cfg.channels.teams.app_password = v;
        cfg.channels.teams.enabled = true;
    }

    // Google Chat
    if let Ok(v) = std::env::var("GOOGLE_CHAT_SERVICE_ACCOUNT_KEY") {
        cfg.channels.google_chat.service_account_key = v;
        cfg.channels.google_chat.enabled = true;
    }
    if let Ok(v) = std::env::var("GOOGLE_CHAT_WEBHOOK_TOKEN") {
        cfg.channels.google_chat.webhook_token = v;
    }

    // Matrix
    if let Ok(v) = std::env::var("MATRIX_HOMESERVER") {
        cfg.channels.matrix.homeserver = v;
    }
    if let Ok(v) = std::env::var("MATRIX_USER_ID") {
        cfg.channels.matrix.user_id = v;
    }
    if let Ok(v) = std::env::var("MATRIX_ACCESS_TOKEN") {
        cfg.channels.matrix.access_token = v;
        cfg.channels.matrix.enabled = true;
    }

    // Zalo
    if let Ok(v) = std::env::var("ZALO_BOT_TOKEN") {
        cfg.channels.zalo.bot_token = v;
        cfg.channels.zalo.enabled = true;
    }
    if let Ok(v) = std::env::var("ZALO_SECRET_TOKEN") {
        cfg.channels.zalo.secret_token = v;
    }

    // Agent defaults
    if let Ok(v) = std::env::var("NANOBOT_MODEL") {
        cfg.agents.defaults.model = v;
    }

    cfg
}

/// Get the default configuration file path.
pub fn get_config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".nanobot")
        .join("config.json")
}

/// Get the nanobot data directory.
pub fn get_data_dir() -> PathBuf {
    let path = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".nanobot");
    std::fs::create_dir_all(&path).ok();
    path
}

/// Load configuration from file or create default.
pub fn load_config(config_path: Option<&Path>) -> Config {
    let path = config_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(get_config_path);

    if path.exists() {
        match std::fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str::<Config>(&content) {
                Ok(config) => return config,
                Err(e) => {
                    tracing::warn!("Failed to parse config from {}: {}", path.display(), e);
                    tracing::warn!("Using default configuration.");
                }
            },
            Err(e) => {
                tracing::warn!("Failed to read config from {}: {}", path.display(), e);
                tracing::warn!("Using default configuration.");
            }
        }
    }

    Config::default()
}

/// Save configuration to file.
pub fn save_config(config: &Config, config_path: Option<&Path>) -> std::result::Result<(), ConfigError> {
    let path = config_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(get_config_path);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ConfigError::Invalid(e.to_string()))?;
    }

    let json = serde_json::to_string_pretty(config)?;
    std::fs::write(&path, json).map_err(|e| ConfigError::Invalid(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = Config::default();
        assert_eq!(cfg.agents.defaults.model, "anthropic/claude-opus-4-5");
        assert_eq!(cfg.agents.defaults.max_tokens, 8192);
        assert_eq!(cfg.agents.defaults.max_tool_iterations, 20);
        assert!(!cfg.channels.telegram.enabled);
        assert!(!cfg.channels.discord.enabled);
        assert!(cfg.providers.openrouter.api_key.is_empty());
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let cfg = Config::default();
        let json = serde_json::to_string_pretty(&cfg).unwrap();
        let parsed: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.agents.defaults.model, cfg.agents.defaults.model);
        assert_eq!(parsed.gateway.port, cfg.gateway.port);
    }

    #[test]
    fn test_config_camelcase_compat() {
        let json = r#"{
            "agents": {
                "defaults": {
                    "model": "openai/gpt-4o",
                    "maxTokens": 4096,
                    "maxToolIterations": 10
                }
            },
            "providers": {
                "openai": {
                    "apiKey": "sk-test123"
                }
            }
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.agents.defaults.model, "openai/gpt-4o");
        assert_eq!(cfg.agents.defaults.max_tokens, 4096);
        assert_eq!(cfg.agents.defaults.max_tool_iterations, 10);
        assert_eq!(cfg.providers.openai.api_key, "sk-test123");
    }

    #[test]
    fn test_get_api_key_matching() {
        let mut cfg = Config::default();
        cfg.providers.openai.api_key = "sk-openai".to_string();
        cfg.providers.anthropic.api_key = "sk-anthropic".to_string();

        assert_eq!(cfg.get_api_key(Some("openai/gpt-4o")), Some("sk-openai"));
        assert_eq!(cfg.get_api_key(Some("gpt-4")), Some("sk-openai"));
        assert_eq!(cfg.get_api_key(Some("claude-3-opus")), Some("sk-anthropic"));
    }

    #[test]
    fn test_get_api_key_fallback() {
        let mut cfg = Config::default();
        cfg.providers.groq.api_key = "gsk-test".to_string();

        // Unknown model falls back to first available key
        assert_eq!(cfg.get_api_key(Some("unknown/model")), Some("gsk-test"));
    }

    #[test]
    fn test_get_api_key_none() {
        let cfg = Config::default();
        assert_eq!(cfg.get_api_key(None), None);
    }

    #[test]
    fn test_get_api_base() {
        let cfg = Config::default();
        let base = cfg.get_api_base(Some("openrouter/foo"));
        assert_eq!(base, Some("https://openrouter.ai/api/v1"));
    }

    #[test]
    fn test_workspace_path_expansion() {
        let cfg = Config::default();
        let ws = cfg.workspace_path();
        // Should not start with "~/" after expansion
        assert!(!ws.to_str().unwrap().starts_with("~/"));
    }

    #[test]
    fn test_save_and_load_config() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.json");

        let mut cfg = Config::default();
        cfg.agents.defaults.model = "test-model".to_string();
        save_config(&cfg, Some(&path)).unwrap();

        assert!(path.exists());
        let loaded = load_config(Some(&path));
        assert_eq!(loaded.agents.defaults.model, "test-model");
    }

    #[test]
    fn test_load_config_missing_file() {
        let path = Path::new("/tmp/nonexistent_nanobot_test.json");
        let cfg = load_config(Some(path));
        // Should return default config
        assert_eq!(cfg.agents.defaults.model, "anthropic/claude-opus-4-5");
    }

    #[test]
    fn test_load_config_from_env_full_json() {
        let json = r#"{
            "providers": {
                "anthropic": { "apiKey": "sk-env-test" }
            }
        }"#;
        std::env::set_var("NANOBOT_CONFIG", json);
        let cfg = load_config_from_env();
        assert_eq!(cfg.providers.anthropic.api_key, "sk-env-test");
        std::env::remove_var("NANOBOT_CONFIG");
    }

    #[test]
    fn test_load_config_from_env_individual_vars() {
        // Clear NANOBOT_CONFIG to test individual vars
        std::env::remove_var("NANOBOT_CONFIG");
        std::env::set_var("ANTHROPIC_API_KEY", "sk-from-env");
        std::env::set_var("TELEGRAM_BOT_TOKEN", "tg-token-123");
        std::env::set_var("TELEGRAM_ALLOW_FROM", "user1, user2");

        let cfg = load_config_from_env();
        assert_eq!(cfg.providers.anthropic.api_key, "sk-from-env");
        assert_eq!(cfg.channels.telegram.token, "tg-token-123");
        assert!(cfg.channels.telegram.enabled);
        assert_eq!(cfg.channels.telegram.allow_from, vec!["user1", "user2"]);

        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("TELEGRAM_BOT_TOKEN");
        std::env::remove_var("TELEGRAM_ALLOW_FROM");
    }
}
