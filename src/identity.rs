#[derive(Debug, Clone)]
pub struct Identity {
    pub name: String,
    pub version: String,
    pub personality: String,
    pub capabilities: Vec<String>,
    pub memory: String,
    pub runtime: String,
    pub channels: Vec<String>,
}

impl Default for Identity {
    fn default() -> Self {
        Self {
            name: "nanobot".to_string(),
            version: "2.0.0".to_string(),
            personality: "Curious, proactive, technically precise".to_string(),
            capabilities: vec![
                "CLI interaction".to_string(),
                "Voice UI".to_string(),
                "File operations".to_string(),
                "Shell command execution".to_string(),
                "Web search & fetch".to_string(),
                "Multi-channel messaging".to_string(),
                "Background task management".to_string(),
                "Easter eggs & omikuji".to_string(),
            ],
            memory: "Persistent memory in workspace/memory/".to_string(),
            runtime: "Rust on macOS aarch64".to_string(),
            channels: vec![
                "CLI".to_string(),
                "Voice".to_string(),
                "Web".to_string(),
                "LINE".to_string(),
                "Telegram".to_string(),
                "Discord".to_string(),
                "WhatsApp".to_string(),
                "Teams".to_string(),
                "Slack".to_string(),
            ],
        }
    }
}