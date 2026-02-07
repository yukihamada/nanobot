use std::path::PathBuf;

/// Core error types for nanobot.
#[derive(Debug, thiserror::Error)]
pub enum NanobotError {
    #[error("Configuration error: {0}")]
    Config(#[from] ConfigError),

    #[error("Provider error: {0}")]
    Provider(#[from] ProviderError),

    #[error("Tool error: {0}")]
    Tool(#[from] ToolError),

    #[error("Channel error: {0}")]
    Channel(#[from] ChannelError),

    #[error("Session error: {0}")]
    Session(#[from] SessionError),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Config file not found: {0}")]
    NotFound(PathBuf),

    #[error("Invalid config: {0}")]
    Invalid(String),

    #[error("Failed to parse config: {0}")]
    Parse(#[from] serde_json::Error),

    #[error("No API key configured")]
    NoApiKey,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("API error ({status}): {message}")]
    Api { status: u16, message: String },

    #[error("Failed to parse response: {0}")]
    Parse(String),

    #[error("No API key configured for provider")]
    NoApiKey,

    #[error("{0}")]
    Other(String),
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("Tool not found: {0}")]
    NotFound(String),

    #[error("Invalid parameters: {0}")]
    InvalidParams(String),

    #[error("Execution error: {0}")]
    Execution(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Timeout after {0} seconds")]
    Timeout(u64),
}

#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error("Connection error: {0}")]
    Connection(String),

    #[error("Authentication error: {0}")]
    Auth(String),

    #[error("Send error: {0}")]
    Send(String),

    #[error("{0}")]
    Other(String),
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("Failed to read session: {0}")]
    Read(String),

    #[error("Failed to write session: {0}")]
    Write(String),

    #[error("Invalid session key: {0}")]
    InvalidKey(String),
}

pub type Result<T> = std::result::Result<T, NanobotError>;
