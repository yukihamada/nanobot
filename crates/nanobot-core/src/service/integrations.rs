//! External API integrations for chatweb.ai
//!
//! Provides tool definitions and execution for external services.
//! Each integration registers tools that the AI can call during conversations.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Tool trait + ToolRegistry — unified interface for built-in and MCP tools
// ---------------------------------------------------------------------------

/// A tool that can be called by the LLM during a conversation.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name (matches function name in tool_call).
    fn name(&self) -> &str;
    /// Human-readable description for the LLM.
    fn description(&self) -> &str;
    /// JSON Schema for parameters.
    fn parameters(&self) -> serde_json::Value;
    /// Execute the tool with the given arguments.
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String;

    /// Return OpenAI function-calling format definition.
    fn to_openai_definition(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": self.description(),
                "parameters": self.parameters(),
            }
        })
    }
}

/// Registry holding all available tools (built-in + MCP).
pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolRegistry {
    /// Create a new registry with only the built-in tools.
    pub fn with_builtins() -> Self {
        #[allow(unused_mut)]
        let mut tools: Vec<Box<dyn Tool>> = vec![
            Box::new(WebSearchTool),
            Box::new(WebFetchTool),
            Box::new(CalculatorTool),
            Box::new(WeatherTool),
            Box::new(TranslateTool),
            Box::new(WikipediaTool),
            Box::new(DateTimeTool),
            Box::new(QrCodeTool),
            Box::new(NewsSearchTool),
            Box::new(GoogleCalendarTool),
            Box::new(GmailTool),
            // Sandbox tools — code execution and file operations
            Box::new(CodeExecuteTool),
            Box::new(SandboxFileReadTool),
            Box::new(SandboxFileWriteTool),
            Box::new(SandboxFileListTool),
            // Media generation tools
            Box::new(ImageGenerateTool),
            Box::new(MusicGenerateTool),
            Box::new(VideoGenerateTool),
            // Webhook / IoT tools
            Box::new(WebhookTriggerTool),
            // Always-on analysis and utility tools
            Box::new(CsvAnalysisTool),
            Box::new(FilesystemTool),
            Box::new(BrowserTool),
            // Git operations tools
            Box::new(GitStatusTool),
            Box::new(GitDiffTool),
            Box::new(GitCommitTool),
            // Quality assurance tools
            Box::new(RunLinterTool),
            Box::new(RunTestsTool),
        ];

        // Register GitHub tools (read works on public repos without token)
        #[cfg(feature = "http-api")]
        {
            tools.push(Box::new(GitHubReadFileTool));
            // Write tools require GITHUB_TOKEN
            if std::env::var("GITHUB_TOKEN").map(|t| !t.is_empty()).unwrap_or(false) {
                tracing::info!("Registering GitHub write tools (GITHUB_TOKEN present)");
                tools.push(Box::new(GitHubCreateOrUpdateFileTool));
                tools.push(Box::new(GitHubCreatePrTool));
            }

            // Phone call tool requires Amazon Connect configuration
            if std::env::var("CONNECT_INSTANCE_ID").map(|v| !v.is_empty()).unwrap_or(false) {
                tracing::info!("Registering phone_call tool (CONNECT_INSTANCE_ID present)");
                tools.push(Box::new(PhoneCallTool));
            }

            // Web deploy tool requires S3 bucket configuration
            if std::env::var("SITES_S3_BUCKET").map(|v| !v.is_empty()).unwrap_or(false) {
                tracing::info!("Registering web_deploy tool (SITES_S3_BUCKET present)");
                tools.push(Box::new(WebDeployTool));
            }

            // Slack tool requires SLACK_BOT_TOKEN
            if std::env::var("SLACK_BOT_TOKEN").map(|v| !v.is_empty()).unwrap_or(false) {
                tracing::info!("Registering slack tool (SLACK_BOT_TOKEN present)");
                tools.push(Box::new(SlackTool));
            }

            // Notion tool requires NOTION_API_KEY
            if std::env::var("NOTION_API_KEY").map(|v| !v.is_empty()).unwrap_or(false) {
                tracing::info!("Registering notion tool (NOTION_API_KEY present)");
                tools.push(Box::new(NotionTool));
            }

            // Discord tool requires DISCORD_WEBHOOK_URL
            if std::env::var("DISCORD_WEBHOOK_URL").map(|v| !v.is_empty()).unwrap_or(false) {
                tracing::info!("Registering discord tool (DISCORD_WEBHOOK_URL present)");
                tools.push(Box::new(DiscordTool));
            }

            // Spotify tool requires SPOTIFY_CLIENT_ID
            if std::env::var("SPOTIFY_CLIENT_ID").map(|v| !v.is_empty()).unwrap_or(false) {
                tracing::info!("Registering spotify tool (SPOTIFY_CLIENT_ID present)");
                tools.push(Box::new(SpotifyTool));
            }

            // Postgres tool requires POSTGRES_URL
            if std::env::var("POSTGRES_URL").map(|v| !v.is_empty()).unwrap_or(false) {
                tracing::info!("Registering postgres tool (POSTGRES_URL present)");
                tools.push(Box::new(PostgresTool));
            }

            // YouTube transcript and arXiv are always available (no API key needed)
            tools.push(Box::new(YouTubeTranscriptTool));
            tools.push(Box::new(ArxivSearchTool));
        }

        Self { tools }
    }

    /// Register additional tools (e.g. from MCP servers).
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Register multiple tools at once.
    pub fn register_all(&mut self, tools: Vec<Box<dyn Tool>>) {
        self.tools.extend(tools);
    }

    /// Get all tool definitions in OpenAI function-calling format.
    pub fn get_definitions(&self) -> Vec<serde_json::Value> {
        self.tools.iter().map(|t| t.to_openai_definition()).collect()
    }

    /// Execute a tool by name with a 10-second timeout.
    pub async fn execute(&self, name: &str, arguments: &HashMap<String, serde_json::Value>) -> String {
        for tool in &self.tools {
            if tool.name() == name {
                match tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    tool.execute(arguments.clone()),
                ).await {
                    Ok(result) => return result,
                    Err(_) => return format!("[TOOL_ERROR] Tool '{name}' timed out after 10s"),
                }
            }
        }
        format!("[TOOL_ERROR] Unknown tool: {name}")
    }

    /// Number of registered tools.
    /// Get all registered tool names as strings.
    pub fn list_tool_names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name().to_string()).collect()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Built-in Tool trait implementations
// ---------------------------------------------------------------------------

/// Web search tool.
pub struct WebSearchTool;

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str { "web_search" }
    fn description(&self) -> &str {
        "Search the web for current information. ALWAYS use this tool when the user asks about prices, products, recent events, news, comparisons, or anything that requires up-to-date data. Returns titles, URLs, and snippets from real web pages. You can then use web_fetch to read specific pages for more detail."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "The search query" }
            },
            "required": ["query"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("no query");
        execute_web_search(query).await
    }
}

/// Web fetch tool.
pub struct WebFetchTool;

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str { "web_fetch" }
    fn description(&self) -> &str {
        "Fetch and read the content of a web page URL. Use this after web_search to get detailed content from specific pages (e.g., product pages for prices, articles for full text). Also use when the user provides a specific URL."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "The URL to fetch" }
            },
            "required": ["url"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let url = params.get("url").and_then(|v| v.as_str()).unwrap_or("");
        execute_web_fetch(url).await
    }
}

/// Calculator tool.
pub struct CalculatorTool;

#[async_trait]
impl Tool for CalculatorTool {
    fn name(&self) -> &str { "calculator" }
    fn description(&self) -> &str {
        "Perform mathematical calculations. Use for arithmetic, conversions, or any math the user asks about."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "expression": { "type": "string", "description": "The mathematical expression to evaluate (e.g., '2 + 3 * 4', '100 * 0.08')" }
            },
            "required": ["expression"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let expr = params.get("expression").and_then(|v| v.as_str()).unwrap_or("0");
        execute_calculator(expr)
    }
}

/// Weather tool.
pub struct WeatherTool;

#[async_trait]
impl Tool for WeatherTool {
    fn name(&self) -> &str { "weather" }
    fn description(&self) -> &str {
        "Get current weather information for a location."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "location": { "type": "string", "description": "City name or location (e.g., 'Tokyo', 'New York')" }
            },
            "required": ["location"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let location = params.get("location").and_then(|v| v.as_str()).unwrap_or("Tokyo");
        execute_weather(location).await
    }
}

/// Translate tool.
pub struct TranslateTool;

#[async_trait]
impl Tool for TranslateTool {
    fn name(&self) -> &str { "translate" }
    fn description(&self) -> &str {
        "Translate text between languages using MyMemory API. Free, no API key required."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": { "type": "string", "description": "Text to translate" },
                "from": { "type": "string", "description": "Source language code (e.g., 'ja', 'en', 'zh', 'ko', 'fr', 'de')" },
                "to": { "type": "string", "description": "Target language code (e.g., 'en', 'ja', 'zh', 'ko', 'fr', 'de')" }
            },
            "required": ["text", "from", "to"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let from = params.get("from").and_then(|v| v.as_str()).unwrap_or("auto");
        let to = params.get("to").and_then(|v| v.as_str()).unwrap_or("en");
        execute_translate(text, from, to).await
    }
}

/// Wikipedia tool.
pub struct WikipediaTool;

#[async_trait]
impl Tool for WikipediaTool {
    fn name(&self) -> &str { "wikipedia" }
    fn description(&self) -> &str {
        "Search Wikipedia and get article summaries. Good for factual information, definitions, and background knowledge."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "The topic to search for" },
                "lang": { "type": "string", "description": "Wikipedia language code (e.g., 'en', 'ja'). Default: 'en'" }
            },
            "required": ["query"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let lang = params.get("lang").and_then(|v| v.as_str()).unwrap_or("en");
        execute_wikipedia(query, lang).await
    }
}

/// DateTime tool.
pub struct DateTimeTool;

#[async_trait]
impl Tool for DateTimeTool {
    fn name(&self) -> &str { "datetime" }
    fn description(&self) -> &str {
        "Get current date and time in any timezone, or convert between timezones."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "timezone": { "type": "string", "description": "Timezone (e.g., 'Asia/Tokyo', 'America/New_York', 'Europe/London', 'UTC'). Default: UTC" }
            },
            "required": []
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let tz = params.get("timezone").and_then(|v| v.as_str()).unwrap_or("UTC");
        execute_datetime(tz)
    }
}

/// URL Shortener / QR Code tool.
pub struct QrCodeTool;

#[async_trait]
impl Tool for QrCodeTool {
    fn name(&self) -> &str { "qr_code" }
    fn description(&self) -> &str {
        "Generate a QR code image URL for any text or URL."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "data": { "type": "string", "description": "The text or URL to encode in the QR code" },
                "size": { "type": "integer", "description": "QR code size in pixels (100-1000). Default: 300" }
            },
            "required": ["data"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let data = params.get("data").and_then(|v| v.as_str()).unwrap_or("");
        let size = params.get("size").and_then(|v| v.as_i64()).unwrap_or(300);
        let size = size.clamp(100, 1000);
        format!(
            "QR Code generated:\nURL: https://api.qrserver.com/v1/create-qr-code/?size={}x{}&data={}\n\nYou can share this URL to display the QR code.",
            size, size, urlencoding::encode(data)
        )
    }
}

/// Brave News Search tool.
pub struct NewsSearchTool;

#[async_trait]
impl Tool for NewsSearchTool {
    fn name(&self) -> &str { "news_search" }
    fn description(&self) -> &str {
        "Search for recent news articles. Use this for breaking news, current events, and trending topics."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "News search query" },
                "freshness": { "type": "string", "description": "How recent: 'pd' (24h), 'pw' (7 days), 'pm' (30 days). Default: 'pw'" }
            },
            "required": ["query"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let freshness = params.get("freshness").and_then(|v| v.as_str()).unwrap_or("pw");
        execute_news_search(query, freshness).await
    }
}

// ---------------------------------------------------------------------------
// Google Calendar tool — list/create events via user's OAuth refresh token
// ---------------------------------------------------------------------------

pub struct GoogleCalendarTool;

#[async_trait]
impl Tool for GoogleCalendarTool {
    fn name(&self) -> &str { "google_calendar" }
    fn description(&self) -> &str {
        "Access the user's Google Calendar. Can list upcoming events or create new events. \
         Requires the user to be logged in with Google OAuth."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "create"],
                    "description": "Action to perform: 'list' upcoming events or 'create' a new event"
                },
                "summary": { "type": "string", "description": "Event title (for create)" },
                "start": { "type": "string", "description": "Start datetime in ISO 8601 format, e.g. '2025-03-01T10:00:00+09:00' (for create)" },
                "end": { "type": "string", "description": "End datetime in ISO 8601 format (for create)" },
                "description": { "type": "string", "description": "Event description (for create, optional)" },
                "max_results": { "type": "integer", "description": "Max events to return (for list, default 10)" }
            },
            "required": ["action"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let action = params.get("action").and_then(|v| v.as_str()).unwrap_or("list");
        let refresh_token = params.get("_refresh_token").and_then(|v| v.as_str()).map(|s| s.to_string());
        execute_google_calendar(action, &params, refresh_token.as_deref()).await
    }
}

// ---------------------------------------------------------------------------
// Gmail tool — search/read/send emails via user's OAuth refresh token
// ---------------------------------------------------------------------------

pub struct GmailTool;

#[async_trait]
impl Tool for GmailTool {
    fn name(&self) -> &str { "gmail" }
    fn description(&self) -> &str {
        "Access the user's Gmail. Can search emails, read a specific email, or send an email. \
         Requires the user to be logged in with Google OAuth."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["search", "read", "send"],
                    "description": "Action: 'search' emails, 'read' a specific email by ID, or 'send' an email"
                },
                "query": { "type": "string", "description": "Search query for Gmail (for search). Uses Gmail search syntax, e.g. 'from:user@example.com' or 'is:unread'" },
                "message_id": { "type": "string", "description": "Gmail message ID (for read)" },
                "to": { "type": "string", "description": "Recipient email address (for send)" },
                "subject": { "type": "string", "description": "Email subject (for send)" },
                "body": { "type": "string", "description": "Email body text (for send)" },
                "max_results": { "type": "integer", "description": "Max emails to return (for search, default 10)" }
            },
            "required": ["action"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let action = params.get("action").and_then(|v| v.as_str()).unwrap_or("search");
        let refresh_token = params.get("_refresh_token").and_then(|v| v.as_str()).map(|s| s.to_string());
        execute_gmail(action, &params, refresh_token.as_deref()).await
    }
}

// ---------------------------------------------------------------------------
// GitHub self-edit tools — read/write files and create PRs on yukihamada/nanobot
// Requires http-api feature (for base64 crate). Included in saas feature.
// ---------------------------------------------------------------------------

#[cfg(feature = "http-api")]
const GITHUB_OWNER: &str = "yukihamada";
#[cfg(feature = "http-api")]
const GITHUB_REPO: &str = "nanobot";
#[cfg(feature = "http-api")]
const GITHUB_MAX_FILE_SIZE: usize = 500 * 1024; // 500 KB

/// Helper: get a reqwest client configured for the GitHub API.
#[cfg(feature = "http-api")]
fn github_client_no_auth() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .default_headers({
            let mut h = reqwest::header::HeaderMap::new();
            h.insert("Accept", "application/vnd.github.v3+json".parse().unwrap());
            h.insert("User-Agent", "chatweb-ai/1.0".parse().unwrap());
            h
        })
        .build()
        .unwrap_or_default()
}

#[cfg(feature = "http-api")]
fn github_client(token: &str) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .default_headers({
            let mut h = reqwest::header::HeaderMap::new();
            h.insert("Accept", "application/vnd.github.v3+json".parse().unwrap());
            h.insert("User-Agent", "chatweb-ai/1.0".parse().unwrap());
            h.insert(
                "Authorization",
                format!("Bearer {}", token).parse().map_err(|e| format!("bad token: {}", e))?,
            );
            h
        })
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))
}

/// Read a file from the GitHub repository.
#[cfg(feature = "http-api")]
pub struct GitHubReadFileTool;

#[cfg(feature = "http-api")]
#[async_trait]
impl Tool for GitHubReadFileTool {
    fn name(&self) -> &str { "github_read_file" }
    fn description(&self) -> &str {
        "Read a file from the chatweb.ai source code repository (yukihamada/nanobot). \
         Returns the file content as text. Use this to inspect the current source code \
         before making changes."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path relative to repo root (e.g. 'README.md', 'crates/nanobot-core/src/service/integrations.rs')"
                },
                "ref": {
                    "type": "string",
                    "description": "Git ref (branch/tag/SHA). Default: 'main'"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let token = std::env::var("GITHUB_TOKEN").ok().filter(|t| !t.is_empty());
        let path = match params.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return "Error: 'path' parameter is required.".to_string(),
        };
        let git_ref = params.get("ref").and_then(|v| v.as_str()).unwrap_or("main");

        // Build client — with token if available, without for public repo fallback
        let client = if let Some(ref tok) = token {
            match github_client(tok) {
                Ok(c) => c,
                Err(_) => github_client_no_auth(),
            }
        } else {
            github_client_no_auth()
        };

        let url = format!(
            "https://api.github.com/repos/{}/{}/contents/{}?ref={}",
            GITHUB_OWNER, GITHUB_REPO, path, git_ref
        );

        tracing::info!("github_read_file: GET {}", url);

        match client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status == reqwest::StatusCode::NOT_FOUND {
                    return format!("File not found: {}", path);
                }
                if !status.is_success() {
                    let body = resp.text().await.unwrap_or_default();
                    return format!("GitHub API error (HTTP {}): {}", status, body);
                }
                match resp.json::<serde_json::Value>().await {
                    Ok(data) => {
                        if let Some(content_b64) = data.get("content").and_then(|v| v.as_str()) {
                            // GitHub returns base64 with newlines
                            let cleaned: String = content_b64.chars().filter(|c| !c.is_whitespace()).collect();
                            match base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &cleaned) {
                                Ok(bytes) => {
                                    match String::from_utf8(bytes) {
                                        Ok(text) => {
                                            let size = data.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
                                            format!("File: {} ({} bytes, ref: {})\n\n{}", path, size, git_ref, text)
                                        }
                                        Err(_) => format!("File {} is binary ({} bytes)", path,
                                            data.get("size").and_then(|v| v.as_u64()).unwrap_or(0)),
                                    }
                                }
                                Err(e) => format!("Failed to decode file content: {}", e),
                            }
                        } else {
                            "Unexpected API response (no content field).".to_string()
                        }
                    }
                    Err(e) => format!("Failed to parse GitHub response: {}", e),
                }
            }
            Err(e) => format!("GitHub API request failed: {}", e),
        }
    }
}

/// Create or update a file in the repository on a feature branch.
#[cfg(feature = "http-api")]
pub struct GitHubCreateOrUpdateFileTool;

#[cfg(feature = "http-api")]
#[async_trait]
impl Tool for GitHubCreateOrUpdateFileTool {
    fn name(&self) -> &str { "github_create_or_update_file" }
    fn description(&self) -> &str {
        "Create or update a file in the chatweb.ai source code repository. \
         The change is committed to a feature branch (never to main). \
         Use github_read_file first to get the current content, then modify and write back."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path relative to repo root"
                },
                "content": {
                    "type": "string",
                    "description": "New file content (full file, not a diff)"
                },
                "message": {
                    "type": "string",
                    "description": "Git commit message"
                },
                "branch": {
                    "type": "string",
                    "description": "Branch name to commit to (e.g. 'fix/weather-desc-en'). Must NOT be 'main' or 'master'."
                }
            },
            "required": ["path", "content", "message", "branch"]
        })
    }

    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let token = match std::env::var("GITHUB_TOKEN") {
            Ok(t) if !t.is_empty() => t,
            _ => return "Error: GITHUB_TOKEN is not configured.".to_string(),
        };
        let path = match params.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return "Error: 'path' parameter is required.".to_string(),
        };
        let content = match params.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return "Error: 'content' parameter is required.".to_string(),
        };
        let message = match params.get("message").and_then(|v| v.as_str()) {
            Some(m) => m,
            None => return "Error: 'message' parameter is required.".to_string(),
        };
        let branch = match params.get("branch").and_then(|v| v.as_str()) {
            Some(b) => b,
            None => return "Error: 'branch' parameter is required.".to_string(),
        };

        // Safety: reject writes to main/master
        if branch == "main" || branch == "master" {
            return "Error: Writing to 'main' or 'master' is forbidden. Use a feature branch.".to_string();
        }

        // Size limit
        if content.len() > GITHUB_MAX_FILE_SIZE {
            return format!(
                "Error: Content too large ({} bytes). Maximum is {} bytes.",
                content.len(), GITHUB_MAX_FILE_SIZE
            );
        }

        let client = match github_client(&token) {
            Ok(c) => c,
            Err(e) => return e,
        };

        // Step 1: Ensure branch exists (create from main HEAD if not)
        let branch_url = format!(
            "https://api.github.com/repos/{}/{}/git/ref/heads/{}",
            GITHUB_OWNER, GITHUB_REPO, branch
        );
        let branch_exists = match client.get(&branch_url).send().await {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        };

        if !branch_exists {
            tracing::info!("github_create_or_update_file: creating branch '{}'", branch);
            // Get main HEAD SHA
            let main_ref_url = format!(
                "https://api.github.com/repos/{}/{}/git/ref/heads/main",
                GITHUB_OWNER, GITHUB_REPO
            );
            let main_sha = match client.get(&main_ref_url).send().await {
                Ok(resp) => {
                    if !resp.status().is_success() {
                        return "Error: Could not get main branch HEAD.".to_string();
                    }
                    match resp.json::<serde_json::Value>().await {
                        Ok(data) => match data.pointer("/object/sha").and_then(|v| v.as_str()) {
                            Some(sha) => sha.to_string(),
                            None => return "Error: Could not parse main branch SHA.".to_string(),
                        },
                        Err(e) => return format!("Error parsing main ref: {}", e),
                    }
                }
                Err(e) => return format!("Error fetching main ref: {}", e),
            };

            // Create the branch
            let create_ref_url = format!(
                "https://api.github.com/repos/{}/{}/git/refs",
                GITHUB_OWNER, GITHUB_REPO
            );
            let create_body = serde_json::json!({
                "ref": format!("refs/heads/{}", branch),
                "sha": main_sha
            });
            match client.post(&create_ref_url).json(&create_body).send().await {
                Ok(resp) => {
                    if !resp.status().is_success() {
                        let body = resp.text().await.unwrap_or_default();
                        return format!("Error creating branch '{}': {}", branch, body);
                    }
                }
                Err(e) => return format!("Error creating branch: {}", e),
            }
        }

        // Step 2: Get existing file SHA (for updates)
        let file_url = format!(
            "https://api.github.com/repos/{}/{}/contents/{}?ref={}",
            GITHUB_OWNER, GITHUB_REPO, path, branch
        );
        let existing_sha = match client.get(&file_url).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    resp.json::<serde_json::Value>().await.ok()
                        .and_then(|d| d.get("sha").and_then(|v| v.as_str()).map(|s| s.to_string()))
                } else {
                    None
                }
            }
            Err(_) => None,
        };

        // Step 3: Create/update file
        let put_url = format!(
            "https://api.github.com/repos/{}/{}/contents/{}",
            GITHUB_OWNER, GITHUB_REPO, path
        );
        let content_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, content.as_bytes());
        let mut put_body = serde_json::json!({
            "message": message,
            "content": content_b64,
            "branch": branch
        });
        if let Some(sha) = &existing_sha {
            put_body["sha"] = serde_json::json!(sha);
        }

        match client.put(&put_url).json(&put_body).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    let action = if existing_sha.is_some() { "Updated" } else { "Created" };
                    format!(
                        "{} file '{}' on branch '{}' with commit message: \"{}\"",
                        action, path, branch, message
                    )
                } else {
                    let body = resp.text().await.unwrap_or_default();
                    format!("Error writing file (HTTP {}): {}", status, body)
                }
            }
            Err(e) => format!("Error writing file: {}", e),
        }
    }
}

/// Create a pull request from a feature branch.
#[cfg(feature = "http-api")]
pub struct GitHubCreatePrTool;

#[cfg(feature = "http-api")]
#[async_trait]
impl Tool for GitHubCreatePrTool {
    fn name(&self) -> &str { "github_create_pr" }
    fn description(&self) -> &str {
        "Create a GitHub Pull Request from a feature branch to main. \
         The PR will be reviewed and merged by a human. \
         Use after committing changes with github_create_or_update_file."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "PR title (short, descriptive)"
                },
                "body": {
                    "type": "string",
                    "description": "PR description (what changed and why)"
                },
                "branch": {
                    "type": "string",
                    "description": "Source branch name (must already exist with commits)"
                }
            },
            "required": ["title", "body", "branch"]
        })
    }

    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let token = match std::env::var("GITHUB_TOKEN") {
            Ok(t) if !t.is_empty() => t,
            _ => return "Error: GITHUB_TOKEN is not configured.".to_string(),
        };
        let title = match params.get("title").and_then(|v| v.as_str()) {
            Some(t) => t,
            None => return "Error: 'title' parameter is required.".to_string(),
        };
        let body = match params.get("body").and_then(|v| v.as_str()) {
            Some(b) => b,
            None => return "Error: 'body' parameter is required.".to_string(),
        };
        let branch = match params.get("branch").and_then(|v| v.as_str()) {
            Some(b) => b,
            None => return "Error: 'branch' parameter is required.".to_string(),
        };

        let client = match github_client(&token) {
            Ok(c) => c,
            Err(e) => return e,
        };

        let url = format!(
            "https://api.github.com/repos/{}/{}/pulls",
            GITHUB_OWNER, GITHUB_REPO
        );
        let pr_body = serde_json::json!({
            "title": title,
            "body": format!("{}\n\n---\n*Created automatically by chatweb.ai*", body),
            "head": branch,
            "base": "main"
        });

        tracing::info!("github_create_pr: POST {} branch={}", url, branch);

        match client.post(&url).json(&pr_body).send().await {
            Ok(resp) => {
                let status = resp.status();
                match resp.json::<serde_json::Value>().await {
                    Ok(data) => {
                        if status.is_success() || status == reqwest::StatusCode::CREATED {
                            let pr_url = data.get("html_url").and_then(|v| v.as_str()).unwrap_or("unknown");
                            let pr_number = data.get("number").and_then(|v| v.as_u64()).unwrap_or(0);
                            format!(
                                "Pull Request #{} created successfully!\nURL: {}\nTitle: {}\n\nA human reviewer will merge this PR.",
                                pr_number, pr_url, title
                            )
                        } else {
                            let msg = data.get("message").and_then(|v| v.as_str()).unwrap_or("unknown error");
                            format!("Error creating PR (HTTP {}): {}", status, msg)
                        }
                    }
                    Err(e) => format!("Error parsing PR response: {}", e),
                }
            }
            Err(e) => format!("Error creating PR: {}", e),
        }
    }
}

/// Available integration types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationType {
    WebSearch,
    WebFetch,
    Weather,
    Calculator,
    Translate,
    Wikipedia,
    DateTime,
    QrCode,
    NewsSearch,
    Gmail,
    Calendar,
    Notion,
    Slack,
    GitHub,
    Discord,
    Spotify,
    Postgres,
    CsvAnalysis,
    Filesystem,
    Browser,
}

/// An integration connection for a user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Integration {
    pub integration_type: IntegrationType,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub requires_auth: bool,
    pub auth_url: Option<String>,
}

/// Tool definition in OpenAI function-calling format.
pub fn get_tool_definitions() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "web_search",
                "description": "Search the web for current information. ALWAYS use this tool when the user asks about prices, products, recent events, news, comparisons, or anything that requires up-to-date data. Returns titles, URLs, and snippets from real web pages. You can then use web_fetch to read specific pages for more detail.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "The search query"
                        }
                    },
                    "required": ["query"]
                }
            }
        }),
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "web_fetch",
                "description": "Fetch and read the content of a web page URL. Use this after web_search to get detailed content from specific pages (e.g., product pages for prices, articles for full text). Also use when the user provides a specific URL.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "The URL to fetch"
                        }
                    },
                    "required": ["url"]
                }
            }
        }),
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "calculator",
                "description": "Perform mathematical calculations. Use for arithmetic, conversions, or any math the user asks about.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "expression": {
                            "type": "string",
                            "description": "The mathematical expression to evaluate (e.g., '2 + 3 * 4', '100 * 0.08')"
                        }
                    },
                    "required": ["expression"]
                }
            }
        }),
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "weather",
                "description": "Get current weather information for a location.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "location": {
                            "type": "string",
                            "description": "City name or location (e.g., 'Tokyo', 'New York')"
                        }
                    },
                    "required": ["location"]
                }
            }
        }),
    ]
}

/// Execute a tool call and return the result.
pub async fn execute_tool(name: &str, arguments: &HashMap<String, serde_json::Value>) -> String {
    match name {
        "web_search" => {
            let query = arguments.get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("no query");
            execute_web_search(query).await
        }
        "web_fetch" => {
            let url = arguments.get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            execute_web_fetch(url).await
        }
        "calculator" => {
            let expr = arguments.get("expression")
                .and_then(|v| v.as_str())
                .unwrap_or("0");
            execute_calculator(expr)
        }
        "weather" => {
            let location = arguments.get("location")
                .and_then(|v| v.as_str())
                .unwrap_or("Tokyo");
            execute_weather(location).await
        }
        "translate" => {
            let text = arguments.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let from = arguments.get("from").and_then(|v| v.as_str()).unwrap_or("auto");
            let to = arguments.get("to").and_then(|v| v.as_str()).unwrap_or("en");
            execute_translate(text, from, to).await
        }
        "wikipedia" => {
            let query = arguments.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let lang = arguments.get("lang").and_then(|v| v.as_str()).unwrap_or("en");
            execute_wikipedia(query, lang).await
        }
        "datetime" => {
            let tz = arguments.get("timezone").and_then(|v| v.as_str()).unwrap_or("UTC");
            execute_datetime(tz)
        }
        "qr_code" => {
            let data = arguments.get("data").and_then(|v| v.as_str()).unwrap_or("");
            let size = arguments.get("size").and_then(|v| v.as_i64()).unwrap_or(300);
            format!(
                "QR Code: https://api.qrserver.com/v1/create-qr-code/?size={}x{}&data={}",
                size.clamp(100, 1000), size.clamp(100, 1000), urlencoding::encode(data)
            )
        }
        "news_search" => {
            let query = arguments.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let freshness = arguments.get("freshness").and_then(|v| v.as_str()).unwrap_or("pw");
            execute_news_search(query, freshness).await
        }
        _ => format!("Unknown tool: {name}"),
    }
}

/// Web search: try Brave API → Bing HTML → Jina search fallback.
pub(crate) async fn execute_web_search(query: &str) -> String {
    tracing::info!("execute_web_search: query={}", query);

    // Try Brave Search API first (if key is available)
    if let Ok(brave_key) = std::env::var("BRAVE_API_KEY") {
        tracing::info!("web_search: trying Brave API");
        match brave_search(query, &brave_key).await {
            Some(result) => return result,
            None => tracing::warn!("web_search: Brave API returned no results"),
        }
    } else {
        tracing::info!("web_search: BRAVE_API_KEY not set, skipping");
    }

    // Try Bing HTML search (works from most cloud IPs, server-rendered)
    tracing::info!("web_search: trying Bing HTML");
    match bing_search(query).await {
        Some(result) => return result,
        None => tracing::warn!("web_search: Bing returned no results"),
    }

    // Try Jina search as general-purpose fallback
    tracing::info!("web_search: trying Jina search fallback");
    let result = jina_search(query).await;
    if result.starts_with("Search results") {
        return result;
    }

    // Last resort: kakaku.com specific search
    tracing::info!("web_search: trying kakaku.com fallback");
    direct_site_search(query).await
}

/// Search using Bing HTML (server-rendered, no JS needed).
async fn bing_search(query: &str) -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .ok()?;

    let url = format!("https://www.bing.com/search?q={}&setlang=ja", urlencoding::encode(query));
    let resp = client.get(&url)
        .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .header("Accept-Language", "ja,en;q=0.9")
        .header("Accept", "text/html")
        .send()
        .await.ok()?;

    if !resp.status().is_success() {
        tracing::warn!("bing_search: status={}", resp.status());
        return None;
    }

    let body = resp.text().await.ok()?;
    tracing::info!("bing_search: got {} bytes", body.len());

    // Check if Bing returned a CAPTCHA or block page (use specific markers)
    if body.contains("verify you are a human") || body.contains("unusual traffic from your") {
        tracing::warn!("bing_search: detected CAPTCHA/block page");
        return None;
    }

    // Parse Bing results from HTML
    // Results are in <li class="b_algo"> blocks
    let mut results = Vec::new();
    // Try both quoted and unquoted class attribute
    let split_markers = ["class=\"b_algo\"", "class=\\\"b_algo\\\"", "class=b_algo"];
    let mut parts: Vec<&str> = Vec::new();
    for marker in &split_markers {
        parts = body.split(marker).collect();
        if parts.len() > 1 {
            tracing::info!("bing_search: found {} parts with marker '{}'", parts.len() - 1, marker);
            break;
        }
    }
    if parts.len() <= 1 {
        // Log a sample of HTML around common Bing keywords
        let sample = if body.len() > 500 { &body[body.len()/3..body.len()/3+500.min(body.len()-body.len()/3)] } else { &body };
        tracing::warn!("bing_search: no b_algo found, HTML sample: {}", &sample[..200.min(sample.len())]);
        return None;
    }

    for (i, part) in parts.iter().skip(1).take(8).enumerate() {
        let mut title = String::new();
        let mut link = String::new();
        let mut snippet = String::new();

        // Extract title from <h2><a>...</a></h2>
        if let Some(h2_start) = part.find("<h2") {
            let h2_rest = &part[h2_start..];
            if let Some(h2_end) = h2_rest.find("</h2>") {
                let h2_block = &h2_rest[..h2_end];
                title = strip_html_tags(h2_block);
            }
        }

        // Extract URL from <cite> tag (clean URL without Bing redirect)
        if let Some(cite_start) = part.find("<cite") {
            let cite_rest = &part[cite_start..];
            if let Some(close) = cite_rest.find('>') {
                let after = &cite_rest[close + 1..];
                if let Some(end) = after.find("</cite>") {
                    link = strip_html_tags(&after[..end]).replace(" › ", "/");
                    // Ensure URL has protocol
                    if !link.starts_with("http") {
                        link = format!("https://{link}");
                    }
                }
            }
        }

        // Extract snippet from <p class="b_lineclamp...">
        if let Some(p_start) = part.find("b_lineclamp") {
            let p_rest = &part[p_start..];
            // Find the opening > of this tag
            if let Some(close) = p_rest.find('>') {
                let after_p = &p_rest[close + 1..];
                if let Some(p_end) = after_p.find("</p>") {
                    snippet = strip_html_tags(&after_p[..p_end]);
                }
            }
        }

        if !title.is_empty() {
            results.push(format!("{}. {}\n   URL: {}\n   {}", i + 1, title.trim(), link.trim(), snippet.trim()));
        }
    }

    tracing::info!("bing_search: extracted {} results", results.len());

    if results.is_empty() {
        None
    } else {
        Some(format!("Search results for \"{}\":\n\n{}", query, results.join("\n\n")))
    }
}

/// Search using Brave Search API (returns high-quality results with URLs).
async fn brave_search(query: &str, api_key: &str) -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;
    let url = format!(
        "https://api.search.brave.com/res/v1/web/search?q={}&count=8",
        urlencoding::encode(query)
    );

    let resp = match client.get(&url)
        .header("Accept", "application/json")
        .header("X-Subscription-Token", api_key)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("brave_search: request failed: {}", e);
            return None;
        }
    };

    if !resp.status().is_success() {
        tracing::warn!("brave_search: HTTP {}", resp.status());
        return None;
    }

    let data: serde_json::Value = match resp.json().await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("brave_search: JSON parse error: {}", e);
            return None;
        }
    };
    let mut results = Vec::new();

    if let Some(web) = data.get("web").and_then(|v| v.get("results")).and_then(|v| v.as_array()) {
        for (i, r) in web.iter().take(8).enumerate() {
            let title = r.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let url = r.get("url").and_then(|v| v.as_str()).unwrap_or("");
            let desc = r.get("description").and_then(|v| v.as_str()).unwrap_or("");
            if !title.is_empty() {
                results.push(format!("{}. {} - {}\n   URL: {}\n   {}", i + 1, title, url, url, desc));
            }
        }
    }

    if results.is_empty() {
        None
    } else {
        Some(format!("Search results for \"{}\":\n\n{}", query, results.join("\n\n")))
    }
}

/// General-purpose search fallback using Jina Reader + DuckDuckGo lite.
async fn jina_search(query: &str) -> String {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    // Use Jina Reader to fetch DuckDuckGo lite (server-rendered, no JS)
    let ddg_url = format!("https://lite.duckduckgo.com/lite/?q={}", urlencoding::encode(query));
    let jina_url = format!("https://r.jina.ai/{ddg_url}");

    tracing::info!("jina_search: fetching {}", jina_url);

    match client.get(&jina_url)
        .header("Accept", "text/plain")
        .header("X-Return-Format", "text")
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            if !status.is_success() {
                tracing::warn!("jina_search: HTTP {} from Jina", status);
                return format!("Search temporarily unavailable (HTTP {status}). Try asking in a different way.");
            }
            match resp.text().await {
                Ok(body) => {
                    tracing::info!("jina_search: got {} bytes", body.len());
                    let useful: String = body.lines()
                        .map(|l| l.trim())
                        .filter(|l| {
                            l.len() > 10 && l.len() < 500
                            && !l.starts_with("![") && !l.starts_with("[Image")
                            && !l.starts_with("[![")
                            && !l.contains("DuckDuckGo") && !l.contains("privacy")
                        })
                        .take(30)
                        .collect::<Vec<_>>()
                        .join("\n");
                    if useful.len() > 50 {
                        let snippet = if useful.len() > 4000 { &useful[..4000] } else { &useful };
                        return format!("Search results for \"{query}\":\n\n{snippet}");
                    }
                    tracing::warn!("jina_search: response too small ({} useful chars)", useful.len());
                    format!("No results found for \"{query}\". Try a more specific query.")
                }
                Err(e) => {
                    tracing::warn!("jina_search: body read error: {}", e);
                    format!("Search error: {e}")
                }
            }
        }
        Err(e) => {
            tracing::warn!("jina_search: request failed: {}", e);
            format!("Search unavailable: {e}")
        }
    }
}

/// Fallback search: fetch kakaku.com search → find product page → fetch actual prices.
async fn direct_site_search(query: &str) -> String {
    // Clean query: strip operators, dates, year references, and noise words
    let clean_query: String = query.split_whitespace()
        .filter(|w| {
            let w_lower = w.to_lowercase();
            !w.starts_with("site:") && !w.starts_with("-")
            && !w.parse::<u32>().map(|n| (2020..=2030).contains(&n)).unwrap_or(false)
            && !w_lower.contains("2024") && !w_lower.contains("2025") && !w_lower.contains("2026")
            && !w_lower.contains("年") && !w_lower.contains("月")
            && !["price", "最安値", "比較", "値段", "cheapest", "lowest", "best", "current"].contains(&w_lower.as_str())
        })
        .collect::<Vec<_>>()
        .join(" ");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(12))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    // Step 1: Fetch kakaku.com search to find product page URLs
    let ascii_query = clean_query.split_whitespace().collect::<Vec<_>>().join("+");
    let search_url = format!("https://search.kakaku.com/{}/", urlencoding::encode(&ascii_query));
    let jina_url = format!("https://r.jina.ai/{search_url}");

    tracing::info!("direct_site_search step1: {}", jina_url);

    let mut product_url: Option<String> = None;
    let mut search_content = String::new();

    if let Ok(resp) = client.get(&jina_url).header("Accept", "text/plain").send().await {
        if resp.status().is_success() {
            if let Ok(body) = resp.text().await {
                // Extract kakaku.com product page URLs (e.g. /model/M0000001094/)
                let mut found_urls = Vec::new();
                for line in body.lines() {
                    if let Some(start) = line.find("https://kakaku.com/") {
                        let rest = &line[start..];
                        let end = rest.find(')').or_else(|| rest.find(' ')).or_else(|| rest.find('"')).unwrap_or(rest.len());
                        let url = &rest[..end];
                        if url.contains("/model/M") || url.contains("/item/") {
                            found_urls.push(url.to_string());
                            if product_url.is_none() {
                                product_url = Some(url.to_string());
                            }
                        }
                    }
                }
                tracing::info!("direct_site_search: found {} product URLs: {:?}", found_urls.len(), found_urls.iter().take(3).collect::<Vec<_>>());
                search_content = body;
            }
        }
    }

    // Step 2: If we found a product page URL, fetch it for actual prices
    if let Some(ref purl) = product_url {
        tracing::info!("direct_site_search step2: fetching product page {}", purl);
        let product_jina = format!("https://r.jina.ai/{purl}");
        if let Ok(resp) = client.get(&product_jina).header("Accept", "text/plain").send().await {
            if resp.status().is_success() {
                if let Ok(body) = resp.text().await {
                    let useful: String = body.lines()
                        .map(|l| l.trim())
                        .filter(|l| {
                            l.len() > 8 && l.len() < 400
                            && !l.starts_with("![") && !l.starts_with("[Image")
                            && !l.starts_with("[![")
                        })
                        .take(80)
                        .collect::<Vec<_>>()
                        .join("\n");
                    if useful.len() > 100 {
                        tracing::info!("direct_site_search: product page {} chars", useful.len());
                        let snippet = if useful.len() > 5000 { &useful[..5000] } else { &useful };
                        return format!("Product details from kakaku.com for \"{query}\":\nURL: {purl}\n\n{snippet}");
                    }
                }
            }
        }
    }

    // Fallback: return whatever we got from search
    if !search_content.is_empty() {
        let useful: String = search_content.lines()
            .map(|l| l.trim())
            .filter(|l| l.len() > 8 && l.len() < 400 && !l.starts_with("![") && !l.starts_with("[Image"))
            .take(40)
            .collect::<Vec<_>>()
            .join("\n");
        if useful.len() > 100 {
            let snippet = if useful.len() > 3000 { &useful[..3000] } else { &useful };
            return format!("Search results from kakaku.com for \"{query}\":\n\n{snippet}");
        }
    }

    format!(
        "Web search is limited. For pricing, try these URLs with web_fetch:\n\
        - kakaku.com: https://search.kakaku.com/{enc}/\n\
        - Apple Store: https://www.apple.com/jp/shop/buy-iphone",
        enc = urlencoding::encode(&ascii_query)
    )
}

/// Fetch a web page via Jina Reader for JS rendering, with fallback to direct fetch.
pub(crate) async fn execute_web_fetch(url: &str) -> String {
    if url.is_empty() {
        return "No URL provided".to_string();
    }

    tracing::info!("web_fetch: url={}", url);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    // Try Jina Reader first for JS rendering
    let jina_url = format!("https://r.jina.ai/{url}");
    tracing::info!("web_fetch: trying Jina Reader");
    match client.get(&jina_url)
        .header("Accept", "text/plain")
        .header("X-Return-Format", "text")
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            if status.is_success() {
                match resp.text().await {
                    Ok(body) => {
                        tracing::info!("web_fetch: Jina returned {} bytes", body.len());
                        let cleaned: String = body.lines()
                            .map(|l| l.trim())
                            .filter(|l| !l.is_empty() && !l.starts_with("!["))
                            .collect::<Vec<_>>()
                            .join("\n");
                        if cleaned.len() > 100 {
                            let snippet = if cleaned.len() > 8000 { &cleaned[..8000] } else { &cleaned };
                            return format!("Content from {url}:\n\n{snippet}");
                        }
                        tracing::warn!("web_fetch: Jina response too small ({} chars), trying direct", cleaned.len());
                    }
                    Err(e) => tracing::warn!("web_fetch: Jina body read error: {}", e),
                }
            } else {
                tracing::warn!("web_fetch: Jina HTTP {}", status);
            }
        }
        Err(e) => tracing::warn!("web_fetch: Jina request failed: {}", e),
    }

    // Fallback: direct fetch with HTML stripping
    tracing::info!("web_fetch: trying direct fetch");
    match client.get(url)
        .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36")
        .header("Accept-Language", "ja,en;q=0.9")
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                tracing::warn!("web_fetch: direct fetch HTTP {} for {}", resp.status(), url);
                return format!("HTTP {} for {}", resp.status(), url);
            }
            match resp.text().await {
                Ok(body) => {
                    tracing::info!("web_fetch: direct fetch got {} bytes", body.len());
                    let text = strip_html_tags(&body);
                    let cleaned: String = text.lines()
                        .map(|l| l.trim())
                        .filter(|l| !l.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n");
                    if cleaned.len() > 8000 {
                        format!("Content from {}:\n\n{}...\n\n[Truncated]", url, &cleaned[..8000])
                    } else {
                        format!("Content from {url}:\n\n{cleaned}")
                    }
                }
                Err(e) => format!("Failed to read page: {e}"),
            }
        }
        Err(e) => format!("Failed to fetch URL: {e}"),
    }
}

/// Simple calculator using basic expression parsing.
fn execute_calculator(expression: &str) -> String {
    // Simple expression evaluator for basic arithmetic
    let expr = expression.replace(' ', "");

    // Try to evaluate as a simple expression
    match eval_simple_expr(&expr) {
        Some(result) => format!("{expression} = {result}"),
        None => format!("Could not evaluate: {expression}"),
    }
}

/// Weather using Open-Meteo free API (no key required).
async fn execute_weather(location: &str) -> String {
    let client = reqwest::Client::new();

    // Geocode first
    let geo_url = format!(
        "https://geocoding-api.open-meteo.com/v1/search?name={}&count=1&language=en",
        urlencoding::encode(location)
    );

    let geo_resp = match client.get(&geo_url).send().await {
        Ok(r) => r,
        Err(e) => return format!("Geocoding failed: {e}"),
    };

    let geo_data: serde_json::Value = match geo_resp.json().await {
        Ok(d) => d,
        Err(e) => return format!("Failed to parse geocoding: {e}"),
    };

    let results = match geo_data.get("results").and_then(|v| v.as_array()) {
        Some(r) if !r.is_empty() => r,
        _ => return format!("Location not found: {location}"),
    };

    let lat = results[0].get("latitude").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let lon = results[0].get("longitude").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let name = results[0].get("name").and_then(|v| v.as_str()).unwrap_or(location);

    // Get weather
    let weather_url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={lat}&longitude={lon}&current=temperature_2m,relative_humidity_2m,wind_speed_10m,weather_code&timezone=auto"
    );

    let weather_resp = match client.get(&weather_url).send().await {
        Ok(r) => r,
        Err(e) => return format!("Weather fetch failed: {e}"),
    };

    let weather: serde_json::Value = match weather_resp.json().await {
        Ok(d) => d,
        Err(e) => return format!("Failed to parse weather: {e}"),
    };

    let current = match weather.get("current") {
        Some(c) => c,
        None => return "No current weather data available".to_string(),
    };

    let temp = current.get("temperature_2m").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let humidity = current.get("relative_humidity_2m").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let wind = current.get("wind_speed_10m").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let code = current.get("weather_code").and_then(|v| v.as_i64()).unwrap_or(0);

    let condition = match code {
        0 => "Clear sky",
        1..=3 => "Partly cloudy",
        45 | 48 => "Foggy",
        51..=57 => "Drizzle",
        61..=67 => "Rain",
        71..=77 => "Snow",
        80..=82 => "Rain showers",
        85 | 86 => "Snow showers",
        95..=99 => "Thunderstorm",
        _ => "Unknown",
    };

    format!(
        "Weather in {name}:\n- Temperature: {temp:.1}°C\n- Condition: {condition}\n- Humidity: {humidity:.0}%\n- Wind: {wind:.1} km/h"
    )
}

/// Translate text using MyMemory API (free, no key required).
async fn execute_translate(text: &str, from: &str, to: &str) -> String {
    if text.is_empty() {
        return "No text provided to translate.".to_string();
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let url = format!(
        "https://api.mymemory.translated.net/get?q={}&langpair={}|{}",
        urlencoding::encode(text),
        urlencoding::encode(from),
        urlencoding::encode(to)
    );

    match client.get(&url).send().await {
        Ok(resp) => {
            if let Ok(data) = resp.json::<serde_json::Value>().await {
                let translated = data
                    .pointer("/responseData/translatedText")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Translation failed");
                let match_score = data
                    .pointer("/responseData/match")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                format!(
                    "Translation ({} → {}):\n\nOriginal: {}\nTranslated: {}\nConfidence: {:.0}%",
                    from, to, text, translated, match_score * 100.0
                )
            } else {
                "Failed to parse translation response.".to_string()
            }
        }
        Err(e) => format!("Translation error: {e}"),
    }
}

/// Search Wikipedia and get article summaries.
async fn execute_wikipedia(query: &str, lang: &str) -> String {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let url = format!(
        "https://{}.wikipedia.org/api/rest_v1/page/summary/{}",
        lang,
        urlencoding::encode(query)
    );

    match client.get(&url)
        .header("User-Agent", "chatweb.ai/1.0 (https://chatweb.ai)")
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                // Try search API as fallback
                let search_url = format!(
                    "https://{}.wikipedia.org/w/api.php?action=query&list=search&srsearch={}&utf8=&format=json&srlimit=3",
                    lang, urlencoding::encode(query)
                );
                if let Ok(sr) = client.get(&search_url).send().await {
                    if let Ok(data) = sr.json::<serde_json::Value>().await {
                        if let Some(results) = data.pointer("/query/search").and_then(|v| v.as_array()) {
                            let mut output = format!("Wikipedia search results for \"{query}\":\n\n");
                            for (i, r) in results.iter().take(3).enumerate() {
                                let title = r.get("title").and_then(|v| v.as_str()).unwrap_or("");
                                let snippet = r.get("snippet").and_then(|v| v.as_str()).unwrap_or("");
                                let clean = strip_html_tags(snippet);
                                output.push_str(&format!(
                                    "{}. {}\n   https://{}.wikipedia.org/wiki/{}\n   {}\n\n",
                                    i + 1, title, lang, urlencoding::encode(title), clean
                                ));
                            }
                            return output;
                        }
                    }
                }
                return format!("No Wikipedia article found for \"{query}\".");
            }

            match resp.json::<serde_json::Value>().await {
                Ok(data) => {
                    let title = data.get("title").and_then(|v| v.as_str()).unwrap_or(query);
                    let extract = data.get("extract").and_then(|v| v.as_str()).unwrap_or("No summary available.");
                    let url = data.pointer("/content_urls/desktop/page").and_then(|v| v.as_str()).unwrap_or("");
                    format!(
                        "Wikipedia: {title}\n\n{extract}\n\nURL: {url}"
                    )
                }
                Err(_) => format!("Failed to parse Wikipedia response for \"{query}\"."),
            }
        }
        Err(e) => format!("Wikipedia error: {e}"),
    }
}

/// Get current datetime in specified timezone.
fn execute_datetime(tz: &str) -> String {
    let now = chrono::Utc::now();
    // Map common timezone names to UTC offsets
    let offset_hours: i32 = match tz.to_lowercase().as_str() {
        "jst" | "asia/tokyo" | "japan" => 9,
        "kst" | "asia/seoul" | "korea" => 9,
        "cst" | "asia/shanghai" | "china" | "asia/taipei" => 8,
        "ist" | "asia/kolkata" | "india" => 5,  // +5:30 approximated
        "gmt" | "utc" | "europe/london" => 0,
        "cet" | "europe/paris" | "europe/berlin" => 1,
        "eet" | "europe/athens" => 2,
        "est" | "america/new_york" | "us/eastern" => -5,
        "cst_us" | "america/chicago" | "us/central" => -6,
        "mst" | "america/denver" | "us/mountain" => -7,
        "pst" | "america/los_angeles" | "us/pacific" => -8,
        "hst" | "pacific/honolulu" | "hawaii" => -10,
        "aest" | "australia/sydney" => 11,
        "nzst" | "pacific/auckland" => 13,
        _ => {
            // Try to parse as +N or -N
            tz.parse::<i32>().unwrap_or_default()
        }
    };

    let offset = chrono::FixedOffset::east_opt(offset_hours * 3600).unwrap_or_else(|| chrono::FixedOffset::east_opt(0).unwrap());
    let local = now.with_timezone(&offset);

    format!(
        "Current time in {} (UTC{}{}):\n\nDate: {}\nTime: {}\nDay: {}\nUnix timestamp: {}",
        tz,
        if offset_hours >= 0 { "+" } else { "" },
        offset_hours,
        local.format("%Y-%m-%d"),
        local.format("%H:%M:%S"),
        local.format("%A"),
        now.timestamp()
    )
}

/// Search for recent news using Brave Search API with freshness filter.
async fn execute_news_search(query: &str, freshness: &str) -> String {
    if let Ok(brave_key) = std::env::var("BRAVE_API_KEY") {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        let url = format!(
            "https://api.search.brave.com/res/v1/web/search?q={}&count=8&freshness={}",
            urlencoding::encode(query),
            freshness
        );

        match client.get(&url)
            .header("Accept", "application/json")
            .header("X-Subscription-Token", &brave_key)
            .send()
            .await
        {
            Ok(resp) => {
                if let Ok(data) = resp.json::<serde_json::Value>().await {
                    let mut results = Vec::new();
                    if let Some(web) = data.get("web").and_then(|v| v.get("results")).and_then(|v| v.as_array()) {
                        for (i, r) in web.iter().take(8).enumerate() {
                            let title = r.get("title").and_then(|v| v.as_str()).unwrap_or("");
                            let url = r.get("url").and_then(|v| v.as_str()).unwrap_or("");
                            let desc = r.get("description").and_then(|v| v.as_str()).unwrap_or("");
                            let age = r.get("age").and_then(|v| v.as_str()).unwrap_or("");
                            if !title.is_empty() {
                                let age_str = if !age.is_empty() { format!(" [{age}]") } else { String::new() };
                                results.push(format!("{}. {}{}\n   URL: {}\n   {}", i + 1, title, age_str, url, desc));
                            }
                        }
                    }
                    if !results.is_empty() {
                        return format!("News results for \"{}\":\n\n{}", query, results.join("\n\n"));
                    }
                }
            }
            Err(e) => tracing::warn!("news_search: Brave API error: {}", e),
        }
    }

    // Fallback to regular web search with news-focused query
    execute_web_search(&format!("{query} news latest")).await
}

/// Strip HTML tags from text, removing script/style content entirely.
fn strip_html_tags(html: &str) -> String {
    // First, remove <script>...</script> and <style>...</style> blocks entirely
    let mut clean = html.to_string();
    for tag in &["script", "style", "noscript", "svg", "head"] {
        loop {
            let open = format!("<{tag}");
            let close = format!("</{tag}>");
            if let Some(start) = clean.to_lowercase().find(&open) {
                if let Some(end_pos) = clean.to_lowercase()[start..].find(&close) {
                    clean.replace_range(start..start + end_pos + close.len(), " ");
                    continue;
                }
            }
            break;
        }
    }

    let mut result = String::new();
    let mut in_tag = false;
    let mut last_was_space = false;

    for c in clean.chars() {
        if c == '<' {
            in_tag = true;
            continue;
        }
        if c == '>' {
            in_tag = false;
            if !last_was_space {
                result.push(' ');
                last_was_space = true;
            }
            continue;
        }
        if in_tag {
            continue;
        }
        if c.is_whitespace() {
            if !last_was_space {
                result.push(' ');
                last_was_space = true;
            }
        } else {
            result.push(c);
            last_was_space = false;
        }
    }

    result.trim().to_string()
}

/// Simple expression evaluator for basic arithmetic.
fn eval_simple_expr(expr: &str) -> Option<f64> {
    // Handle simple binary operations
    if let Some(i) = expr.rfind('+') {
        if i > 0 {
            let left = eval_simple_expr(&expr[..i])?;
            let right = eval_simple_expr(&expr[i+1..])?;
            return Some(left + right);
        }
    }
    if let Some(i) = expr.rfind('-') {
        if i > 0 {
            let left = eval_simple_expr(&expr[..i])?;
            let right = eval_simple_expr(&expr[i+1..])?;
            return Some(left - right);
        }
    }
    if let Some(i) = expr.rfind('*') {
        let left = eval_simple_expr(&expr[..i])?;
        let right = eval_simple_expr(&expr[i+1..])?;
        return Some(left * right);
    }
    if let Some(i) = expr.rfind('/') {
        let left = eval_simple_expr(&expr[..i])?;
        let right = eval_simple_expr(&expr[i+1..])?;
        if right == 0.0 { return None; }
        return Some(left / right);
    }

    // Try parsing as number
    expr.parse::<f64>().ok()
}

/// List available integrations for display.
pub fn list_integrations() -> Vec<Integration> {
    vec![
        Integration {
            integration_type: IntegrationType::WebSearch,
            name: "Web Search".to_string(),
            description: "Search the web for current information".to_string(),
            enabled: true,
            requires_auth: false,
            auth_url: None,
        },
        Integration {
            integration_type: IntegrationType::WebFetch,
            name: "Web Fetch".to_string(),
            description: "Fetch and read web page content".to_string(),
            enabled: true,
            requires_auth: false,
            auth_url: None,
        },
        Integration {
            integration_type: IntegrationType::Weather,
            name: "Weather".to_string(),
            description: "Get current weather for any location".to_string(),
            enabled: true,
            requires_auth: false,
            auth_url: None,
        },
        Integration {
            integration_type: IntegrationType::Calculator,
            name: "Calculator".to_string(),
            description: "Perform mathematical calculations".to_string(),
            enabled: true,
            requires_auth: false,
            auth_url: None,
        },
        Integration {
            integration_type: IntegrationType::Translate,
            name: "Translate".to_string(),
            description: "Translate text between languages".to_string(),
            enabled: true,
            requires_auth: false,
            auth_url: None,
        },
        Integration {
            integration_type: IntegrationType::Wikipedia,
            name: "Wikipedia".to_string(),
            description: "Search Wikipedia articles and summaries".to_string(),
            enabled: true,
            requires_auth: false,
            auth_url: None,
        },
        Integration {
            integration_type: IntegrationType::DateTime,
            name: "Date & Time".to_string(),
            description: "Get current date/time in any timezone".to_string(),
            enabled: true,
            requires_auth: false,
            auth_url: None,
        },
        Integration {
            integration_type: IntegrationType::QrCode,
            name: "QR Code".to_string(),
            description: "Generate QR codes for any text or URL".to_string(),
            enabled: true,
            requires_auth: false,
            auth_url: None,
        },
        Integration {
            integration_type: IntegrationType::NewsSearch,
            name: "News Search".to_string(),
            description: "Search recent news articles".to_string(),
            enabled: true,
            requires_auth: false,
            auth_url: None,
        },
        Integration {
            integration_type: IntegrationType::Gmail,
            name: "Gmail".to_string(),
            description: "Read and send emails via Gmail".to_string(),
            enabled: false,
            requires_auth: true,
            auth_url: Some("https://accounts.google.com/o/oauth2/v2/auth".to_string()),
        },
        Integration {
            integration_type: IntegrationType::Calendar,
            name: "Google Calendar".to_string(),
            description: "View and create calendar events".to_string(),
            enabled: false,
            requires_auth: true,
            auth_url: Some("https://accounts.google.com/o/oauth2/v2/auth".to_string()),
        },
        Integration {
            integration_type: IntegrationType::Notion,
            name: "Notion".to_string(),
            description: "Read and write Notion pages".to_string(),
            enabled: false,
            requires_auth: true,
            auth_url: Some("https://api.notion.com/v1/oauth/authorize".to_string()),
        },
        Integration {
            integration_type: IntegrationType::Slack,
            name: "Slack".to_string(),
            description: "Send messages to Slack channels".to_string(),
            enabled: false,
            requires_auth: true,
            auth_url: Some("https://slack.com/oauth/v2/authorize".to_string()),
        },
        Integration {
            integration_type: IntegrationType::GitHub,
            name: "GitHub".to_string(),
            description: "Read/write source code and create Pull Requests on yukihamada/nanobot".to_string(),
            enabled: std::env::var("GITHUB_TOKEN").map(|t| !t.is_empty()).unwrap_or(false),
            requires_auth: true,
            auth_url: Some("https://github.com/login/oauth/authorize".to_string()),
        },
        Integration {
            integration_type: IntegrationType::Discord,
            name: "Discord".to_string(),
            description: "Send messages to Discord channels via webhooks".to_string(),
            enabled: std::env::var("DISCORD_WEBHOOK_URL").map(|v| !v.is_empty()).unwrap_or(false),
            requires_auth: true,
            auth_url: None,
        },
        Integration {
            integration_type: IntegrationType::Spotify,
            name: "Spotify".to_string(),
            description: "Search tracks, albums, and artists on Spotify".to_string(),
            enabled: std::env::var("SPOTIFY_CLIENT_ID").map(|v| !v.is_empty()).unwrap_or(false),
            requires_auth: true,
            auth_url: None,
        },
        Integration {
            integration_type: IntegrationType::Postgres,
            name: "PostgreSQL".to_string(),
            description: "Query PostgreSQL databases (read-only)".to_string(),
            enabled: std::env::var("POSTGRES_URL").map(|v| !v.is_empty()).unwrap_or(false),
            requires_auth: true,
            auth_url: None,
        },
        Integration {
            integration_type: IntegrationType::CsvAnalysis,
            name: "CSV Analysis".to_string(),
            description: "Parse and analyze CSV data".to_string(),
            enabled: true,
            requires_auth: false,
            auth_url: None,
        },
        Integration {
            integration_type: IntegrationType::Filesystem,
            name: "Filesystem".to_string(),
            description: "Extended file operations (find, grep, diff) in sandbox".to_string(),
            enabled: true,
            requires_auth: false,
            auth_url: None,
        },
        Integration {
            integration_type: IntegrationType::Browser,
            name: "Browser".to_string(),
            description: "Fetch and extract content from web pages using CSS selectors".to_string(),
            enabled: true,
            requires_auth: false,
            auth_url: None,
        },
    ]
}

// ---------------------------------------------------------------------------
// Google API helpers — refresh token exchange + Calendar/Gmail execution
// ---------------------------------------------------------------------------

/// Refresh a Google OAuth access token using a refresh_token.
async fn google_refresh_access_token(refresh_token: &str) -> Result<String, String> {
    let client_id = std::env::var("GOOGLE_CLIENT_ID").unwrap_or_default();
    let client_secret = std::env::var("GOOGLE_CLIENT_SECRET").unwrap_or_default();
    if client_id.is_empty() || client_secret.is_empty() {
        return Err("Google OAuth not configured".to_string());
    }

    let client = reqwest::Client::new();
    let resp = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .map_err(|e| format!("Token refresh request failed: {e}"))?;

    let data: serde_json::Value = resp.json().await.map_err(|e| format!("Token parse error: {e}"))?;
    data.get("access_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("No access_token in response: {data}"))
}

/// Get user's Google refresh token from DynamoDB (via GOOGLE_SA_KEY env or lookup).
/// This function looks up the refresh token from the user's session.
async fn get_google_refresh_token_for_session() -> Result<String, String> {
    // Try to get the service account key as fallback
    let sa_key = std::env::var("GOOGLE_SA_KEY").unwrap_or_default();
    if !sa_key.is_empty() {
        // Use service account JWT flow
        return get_sa_access_token(&sa_key).await;
    }
    Err("No Google refresh token available. User must log in with Google OAuth first.".to_string())
}

/// Get access token from service account JSON key.
async fn get_sa_access_token(sa_key_json: &str) -> Result<String, String> {
    let key: serde_json::Value = serde_json::from_str(sa_key_json)
        .map_err(|e| format!("Invalid service account key: {e}"))?;

    let client_email = key.get("client_email").and_then(|v| v.as_str()).unwrap_or("");
    let private_key_pem = key.get("private_key").and_then(|v| v.as_str()).unwrap_or("");

    if client_email.is_empty() || private_key_pem.is_empty() {
        return Err("Service account key missing client_email or private_key".to_string());
    }

    // Build JWT
    let now = chrono::Utc::now().timestamp();
    let header = base64_url_encode(&serde_json::json!({"alg":"RS256","typ":"JWT"}).to_string());
    let claims = base64_url_encode(&serde_json::json!({
        "iss": client_email,
        "scope": "https://www.googleapis.com/auth/calendar https://www.googleapis.com/auth/gmail.modify",
        "aud": "https://oauth2.googleapis.com/token",
        "iat": now,
        "exp": now + 3600,
    }).to_string());

    let unsigned = format!("{header}.{claims}");

    // Sign with RSA-SHA256 using the private key
    let signature = sign_rs256(private_key_pem, unsigned.as_bytes())
        .map_err(|e| format!("JWT signing failed: {e}"))?;

    let jwt = format!("{unsigned}.{signature}");

    // Exchange JWT for access token
    let client = reqwest::Client::new();
    let resp = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", &jwt),
        ])
        .send()
        .await
        .map_err(|e| format!("SA token request failed: {e}"))?;

    let data: serde_json::Value = resp.json().await
        .map_err(|e| format!("SA token parse error: {e}"))?;

    data.get("access_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("No access_token in SA response: {data}"))
}

fn base64_url_encode(input: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(input.as_bytes())
}

fn sign_rs256(_pem: &str, _data: &[u8]) -> Result<String, String> {
    // RSA signing requires additional crates. For now, rely on user OAuth tokens.
    Err("RSA signing not available. Use user OAuth tokens instead.".to_string())
}

/// Execute Google Calendar tool.
async fn execute_google_calendar(action: &str, params: &HashMap<String, serde_json::Value>, injected_refresh_token: Option<&str>) -> String {
    // Try to get access token from user's refresh token passed via parameter
    let refresh_token = injected_refresh_token.unwrap_or("").to_string();
    let access_token = if !refresh_token.is_empty() {
        match google_refresh_access_token(&refresh_token).await {
            Ok(t) => t,
            Err(e) => return format!("Error getting Google access token: {e}"),
        }
    } else {
        // Fall back to service account
        match get_google_refresh_token_for_session().await {
            Ok(t) => t,
            Err(e) => return format!("Google Calendar requires login with Google. Error: {e}"),
        }
    };

    let client = reqwest::Client::new();

    match action {
        "list" => {
            let max = params.get("max_results").and_then(|v| v.as_i64()).unwrap_or(10);
            let now = chrono::Utc::now().to_rfc3339();
            let url = format!(
                "https://www.googleapis.com/calendar/v3/calendars/primary/events?maxResults={}&orderBy=startTime&singleEvents=true&timeMin={}",
                max, urlencoding::encode(&now)
            );
            match client.get(&url).bearer_auth(&access_token).send().await {
                Ok(resp) => {
                    match resp.json::<serde_json::Value>().await {
                        Ok(data) => {
                            if let Some(items) = data.get("items").and_then(|v| v.as_array()) {
                                let mut result = format!("Upcoming {} events:\n\n", items.len());
                                for (i, item) in items.iter().enumerate() {
                                    let summary = item.get("summary").and_then(|v| v.as_str()).unwrap_or("(no title)");
                                    let start = item.get("start")
                                        .and_then(|s| s.get("dateTime").or(s.get("date")))
                                        .and_then(|v| v.as_str()).unwrap_or("?");
                                    let end = item.get("end")
                                        .and_then(|s| s.get("dateTime").or(s.get("date")))
                                        .and_then(|v| v.as_str()).unwrap_or("?");
                                    let desc = item.get("description").and_then(|v| v.as_str()).unwrap_or("");
                                    result.push_str(&format!("{}. {} | {} → {}", i + 1, summary, start, end));
                                    if !desc.is_empty() {
                                        result.push_str(&format!("\n   {}", &desc[..desc.len().min(100)]));
                                    }
                                    result.push('\n');
                                }
                                result
                            } else if let Some(err) = data.get("error") {
                                format!("Calendar API error: {err}")
                            } else {
                                "No upcoming events found.".to_string()
                            }
                        }
                        Err(e) => format!("Failed to parse calendar response: {e}"),
                    }
                }
                Err(e) => format!("Calendar request failed: {e}"),
            }
        }
        "create" => {
            let summary = params.get("summary").and_then(|v| v.as_str()).unwrap_or("New Event");
            let start = params.get("start").and_then(|v| v.as_str()).unwrap_or("");
            let end = params.get("end").and_then(|v| v.as_str()).unwrap_or("");
            let description = params.get("description").and_then(|v| v.as_str()).unwrap_or("");

            if start.is_empty() || end.is_empty() {
                return "Error: Both 'start' and 'end' datetimes are required for creating an event.".to_string();
            }

            let event_body = serde_json::json!({
                "summary": summary,
                "description": description,
                "start": { "dateTime": start },
                "end": { "dateTime": end },
            });

            match client
                .post("https://www.googleapis.com/calendar/v3/calendars/primary/events")
                .bearer_auth(&access_token)
                .json(&event_body)
                .send()
                .await
            {
                Ok(resp) => {
                    match resp.json::<serde_json::Value>().await {
                        Ok(data) => {
                            if let Some(id) = data.get("id").and_then(|v| v.as_str()) {
                                let link = data.get("htmlLink").and_then(|v| v.as_str()).unwrap_or("");
                                format!("Event created: '{summary}'\nID: {id}\nLink: {link}")
                            } else if let Some(err) = data.get("error") {
                                format!("Calendar API error: {err}")
                            } else {
                                format!("Event created: {data}")
                            }
                        }
                        Err(e) => format!("Failed to parse create response: {e}"),
                    }
                }
                Err(e) => format!("Calendar create request failed: {e}"),
            }
        }
        _ => format!("Unknown calendar action: '{action}'. Use 'list' or 'create'."),
    }
}

/// Execute Gmail tool.
async fn execute_gmail(action: &str, params: &HashMap<String, serde_json::Value>, injected_refresh_token: Option<&str>) -> String {
    let refresh_token = injected_refresh_token.unwrap_or("").to_string();
    let access_token = if !refresh_token.is_empty() {
        match google_refresh_access_token(&refresh_token).await {
            Ok(t) => t,
            Err(e) => return format!("Error getting Google access token: {e}"),
        }
    } else {
        match get_google_refresh_token_for_session().await {
            Ok(t) => t,
            Err(e) => return format!("Gmail requires login with Google. Error: {e}"),
        }
    };

    let client = reqwest::Client::new();

    match action {
        "search" => {
            let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("is:inbox");
            let max = params.get("max_results").and_then(|v| v.as_i64()).unwrap_or(10);
            let url = format!(
                "https://www.googleapis.com/gmail/v1/users/me/messages?q={}&maxResults={}",
                urlencoding::encode(query), max
            );
            match client.get(&url).bearer_auth(&access_token).send().await {
                Ok(resp) => {
                    let data: serde_json::Value = match resp.json().await {
                        Ok(d) => d,
                        Err(e) => return format!("Gmail parse error: {e}"),
                    };
                    if let Some(messages) = data.get("messages").and_then(|v| v.as_array()) {
                        let mut result = format!("Found {} messages:\n\n", messages.len());
                        // Fetch details for each message (up to max)
                        for msg in messages.iter().take(max as usize) {
                            let msg_id = msg.get("id").and_then(|v| v.as_str()).unwrap_or("");
                            if msg_id.is_empty() { continue; }
                            let detail_url = format!(
                                "https://www.googleapis.com/gmail/v1/users/me/messages/{msg_id}?format=metadata&metadataHeaders=Subject&metadataHeaders=From&metadataHeaders=Date"
                            );
                            if let Ok(detail_resp) = client.get(&detail_url).bearer_auth(&access_token).send().await {
                                if let Ok(detail) = detail_resp.json::<serde_json::Value>().await {
                                    let headers = detail.get("payload")
                                        .and_then(|p| p.get("headers"))
                                        .and_then(|h| h.as_array());
                                    let mut from = "";
                                    let mut subject = "";
                                    let mut date = "";
                                    if let Some(hdrs) = headers {
                                        for h in hdrs {
                                            let name = h.get("name").and_then(|v| v.as_str()).unwrap_or("");
                                            let val = h.get("value").and_then(|v| v.as_str()).unwrap_or("");
                                            match name {
                                                "From" => from = val,
                                                "Subject" => subject = val,
                                                "Date" => date = val,
                                                _ => {}
                                            }
                                        }
                                    }
                                    let snippet = detail.get("snippet").and_then(|v| v.as_str()).unwrap_or("");
                                    result.push_str(&format!("ID: {}\n  From: {}\n  Subject: {}\n  Date: {}\n  Preview: {}\n\n",
                                        msg_id, from, subject, date, &snippet[..snippet.len().min(120)]));
                                }
                            }
                        }
                        result
                    } else if let Some(err) = data.get("error") {
                        format!("Gmail API error: {err}")
                    } else {
                        "No messages found.".to_string()
                    }
                }
                Err(e) => format!("Gmail search failed: {e}"),
            }
        }
        "read" => {
            let msg_id = params.get("message_id").and_then(|v| v.as_str()).unwrap_or("");
            if msg_id.is_empty() {
                return "Error: 'message_id' is required for reading an email.".to_string();
            }
            let url = format!(
                "https://www.googleapis.com/gmail/v1/users/me/messages/{msg_id}?format=full"
            );
            match client.get(&url).bearer_auth(&access_token).send().await {
                Ok(resp) => {
                    match resp.json::<serde_json::Value>().await {
                        Ok(data) => {
                            let headers = data.get("payload")
                                .and_then(|p| p.get("headers"))
                                .and_then(|h| h.as_array());
                            let mut from = String::new();
                            let mut to = String::new();
                            let mut subject = String::new();
                            let mut date = String::new();
                            if let Some(hdrs) = headers {
                                for h in hdrs {
                                    let name = h.get("name").and_then(|v| v.as_str()).unwrap_or("");
                                    let val = h.get("value").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                    match name {
                                        "From" => from = val,
                                        "To" => to = val,
                                        "Subject" => subject = val,
                                        "Date" => date = val,
                                        _ => {}
                                    }
                                }
                            }
                            let snippet = data.get("snippet").and_then(|v| v.as_str()).unwrap_or("");
                            // Try to get body text
                            let body = extract_gmail_body(&data);
                            format!("From: {}\nTo: {}\nSubject: {}\nDate: {}\n\n{}", from, to, subject, date,
                                if body.is_empty() { snippet.to_string() } else { body })
                        }
                        Err(e) => format!("Gmail read parse error: {e}"),
                    }
                }
                Err(e) => format!("Gmail read failed: {e}"),
            }
        }
        "send" => {
            let to = params.get("to").and_then(|v| v.as_str()).unwrap_or("");
            let subject = params.get("subject").and_then(|v| v.as_str()).unwrap_or("");
            let body = params.get("body").and_then(|v| v.as_str()).unwrap_or("");

            if to.is_empty() || subject.is_empty() {
                return "Error: 'to' and 'subject' are required for sending an email.".to_string();
            }

            // Construct RFC 2822 message
            let raw_msg = format!("To: {to}\r\nSubject: {subject}\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}");
            use base64::Engine;
            let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw_msg.as_bytes());

            let send_body = serde_json::json!({ "raw": encoded });

            match client
                .post("https://www.googleapis.com/gmail/v1/users/me/messages/send")
                .bearer_auth(&access_token)
                .json(&send_body)
                .send()
                .await
            {
                Ok(resp) => {
                    match resp.json::<serde_json::Value>().await {
                        Ok(data) => {
                            if let Some(id) = data.get("id").and_then(|v| v.as_str()) {
                                format!("Email sent successfully!\nTo: {to}\nSubject: {subject}\nMessage ID: {id}")
                            } else if let Some(err) = data.get("error") {
                                format!("Gmail send error: {err}")
                            } else {
                                format!("Email sent: {data}")
                            }
                        }
                        Err(e) => format!("Gmail send parse error: {e}"),
                    }
                }
                Err(e) => format!("Gmail send failed: {e}"),
            }
        }
        _ => format!("Unknown Gmail action: '{action}'. Use 'search', 'read', or 'send'."),
    }
}

/// Extract plain text body from Gmail message payload.
fn extract_gmail_body(data: &serde_json::Value) -> String {
    // Try direct body
    if let Some(body_data) = data.get("payload")
        .and_then(|p| p.get("body"))
        .and_then(|b| b.get("data"))
        .and_then(|d| d.as_str())
    {
        use base64::Engine;
        if let Ok(decoded) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(body_data) {
            if let Ok(text) = String::from_utf8(decoded) {
                return text;
            }
        }
    }
    // Try parts
    if let Some(parts) = data.get("payload")
        .and_then(|p| p.get("parts"))
        .and_then(|p| p.as_array())
    {
        for part in parts {
            let mime = part.get("mimeType").and_then(|v| v.as_str()).unwrap_or("");
            if mime == "text/plain" {
                if let Some(body_data) = part.get("body")
                    .and_then(|b| b.get("data"))
                    .and_then(|d| d.as_str())
                {
                    use base64::Engine;
                    if let Ok(decoded) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(body_data) {
                        if let Ok(text) = String::from_utf8(decoded) {
                            return text;
                        }
                    }
                }
            }
        }
    }
    String::new()
}

// ---------------------------------------------------------------------------
// Sandbox tools — code execution and file operations in /tmp/sandbox/
// ---------------------------------------------------------------------------

/// Code execution tool — runs shell commands (with optional Python/Node.js) in a sandbox.
pub struct CodeExecuteTool;

impl CodeExecuteTool {
    /// Check if an interpreter is available on the system.
    fn interpreter_available(cmd: &str) -> bool {
        std::process::Command::new("which")
            .arg(cmd)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Run a command in the sandbox with timeout.
    async fn run_in_sandbox(cmd: &str, args: &[&str], sandbox_dir: &str) -> String {
        std::fs::create_dir_all(sandbox_dir).ok();

        match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            tokio::process::Command::new(cmd)
                .args(args)
                .current_dir(sandbox_dir)
                .env("HOME", sandbox_dir)
                .env("TMPDIR", sandbox_dir)
                .output(),
        ).await {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let mut result = String::new();
                if !stdout.is_empty() {
                    result.push_str(&stdout);
                }
                if !stderr.trim().is_empty() {
                    if !result.is_empty() { result.push('\n'); }
                    result.push_str(&format!("STDERR: {}", stderr));
                }
                if !output.status.success() {
                    if !result.is_empty() { result.push('\n'); }
                    result.push_str(&format!("Exit code: {}", output.status.code().unwrap_or(-1)));
                }
                if result.is_empty() {
                    result = "(no output)".to_string();
                }
                // Truncate long output
                if result.len() > 8000 {
                    let end = result.char_indices().nth(8000).map(|(i, _)| i).unwrap_or(result.len());
                    result = format!("{}...\n[truncated, {} total bytes]", &result[..end], result.len());
                }
                result
            }
            Ok(Err(e)) => format!("[TOOL_ERROR] Failed to execute: {e}"),
            Err(_) => "[TOOL_ERROR] Code execution timed out after 10s".to_string(),
        }
    }
}

#[async_trait]
impl Tool for CodeExecuteTool {
    fn name(&self) -> &str { "code_execute" }
    fn description(&self) -> &str {
        "Execute code in a sandboxed environment. \
         IMPORTANT: Use language='shell' for best compatibility — it works everywhere. \
         Shell supports awk, sed, bc, and standard Unix tools for calculations and text processing. \
         Python and Node.js are available only if installed on the server. \
         Use file_write to create script files, then execute them with shell. \
         The sandbox persists files across calls within the same session. \
         Example shell math: echo $((1+2+3)) or echo '1+2+3' | bc or awk 'BEGIN{for(i=1;i<=100;i++)s+=i;print s}'"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "language": {
                    "type": "string",
                    "enum": ["shell", "python", "nodejs"],
                    "description": "Language to use. 'shell' is always available. 'python' and 'nodejs' may not be installed."
                },
                "code": {
                    "type": "string",
                    "description": "The code to execute"
                }
            },
            "required": ["language", "code"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let language = params.get("language").and_then(|v| v.as_str()).unwrap_or("shell");
        let code = params.get("code").and_then(|v| v.as_str()).unwrap_or("");
        let sandbox_dir = params.get("_sandbox_dir").and_then(|v| v.as_str()).unwrap_or("/tmp/sandbox/default");

        if code.is_empty() {
            return "Error: 'code' parameter is required".to_string();
        }
        if code.len() > 32_000 {
            return "Error: code too large (max 32KB)".to_string();
        }

        // Safety: deny dangerous patterns
        let lower = code.to_lowercase();
        let deny_patterns = [
            "rm -rf /", "rm -rf /*", "mkfs", "dd if=", "> /dev/", "shutdown", "reboot",
            ":(){ :|:& };:", "fork()",
        ];
        for pat in &deny_patterns {
            if lower.contains(pat) {
                return "[TOOL_ERROR] Code blocked by safety guard: dangerous pattern detected".to_string();
            }
        }

        match language {
            "shell" => {
                Self::run_in_sandbox("sh", &["-c", code], sandbox_dir).await
            }
            "python" => {
                // Try python3, then python, then fallback to writing a script and running via sh
                for interpreter in &["python3", "python"] {
                    if Self::interpreter_available(interpreter) {
                        return Self::run_in_sandbox(interpreter, &["-c", code], sandbox_dir).await;
                    }
                }
                // Fallback: write to file and try to run
                let script_path = format!("{}/_.py", sandbox_dir);
                std::fs::create_dir_all(sandbox_dir).ok();
                if let Err(e) = std::fs::write(&script_path, code) {
                    return format!("[TOOL_ERROR] Failed to write script: {e}");
                }
                // Try common Python paths
                for path in &["/usr/bin/python3", "/usr/local/bin/python3", "/opt/python/bin/python3"] {
                    if std::path::Path::new(path).exists() {
                        return Self::run_in_sandbox(path, &[&script_path], sandbox_dir).await;
                    }
                }
                "[TOOL_ERROR] Python is not available on this server. Please use language='shell' instead. \
                 Shell supports awk for calculations: awk 'BEGIN{for(i=0;i<20;i++){if(i<2)a[i]=i;else a[i]=a[i-1]+a[i-2];print a[i]}}'".to_string()
            }
            "nodejs" => {
                if Self::interpreter_available("node") {
                    return Self::run_in_sandbox("node", &["-e", code], sandbox_dir).await;
                }
                "[TOOL_ERROR] Node.js is not available on this server. Please use language='shell' instead.".to_string()
            }
            _ => format!("[TOOL_ERROR] Unsupported language: {language}. Use 'shell', 'python', or 'nodejs'."),
        }
    }
}

/// File read tool — reads files from the session sandbox.
pub struct SandboxFileReadTool;

#[async_trait]
impl Tool for SandboxFileReadTool {
    fn name(&self) -> &str { "file_read" }
    fn description(&self) -> &str {
        "Read the contents of a file in the sandbox. Use after code_execute to inspect generated files."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative file path within the sandbox (e.g., 'output.txt', 'src/main.py')"
                }
            },
            "required": ["path"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let sandbox_dir = params.get("_sandbox_dir").and_then(|v| v.as_str()).unwrap_or("/tmp/sandbox/default");

        if path.is_empty() {
            return "Error: 'path' parameter is required".to_string();
        }
        if path.contains("..") {
            return "[TOOL_ERROR] Path traversal not allowed".to_string();
        }

        let full_path = std::path::Path::new(sandbox_dir).join(path);
        if !full_path.starts_with(sandbox_dir) {
            return "[TOOL_ERROR] Path outside sandbox".to_string();
        }
        if !full_path.exists() {
            return format!("Error: File not found: {path}");
        }
        if !full_path.is_file() {
            return format!("Error: Not a file: {path}");
        }
        match std::fs::read_to_string(&full_path) {
            Ok(content) => {
                if content.len() > 32_000 {
                    let end = content.char_indices().nth(32_000).map(|(i, _)| i).unwrap_or(content.len());
                    format!("{}...\n[truncated, {} total bytes]", &content[..end], content.len())
                } else {
                    content
                }
            }
            Err(e) => format!("Error reading file: {e}"),
        }
    }
}

/// File write tool — writes files into the session sandbox.
pub struct SandboxFileWriteTool;

#[async_trait]
impl Tool for SandboxFileWriteTool {
    fn name(&self) -> &str { "file_write" }
    fn description(&self) -> &str {
        "Write content to a file in the sandbox. Creates parent directories automatically. Max 100KB."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative file path within the sandbox (e.g., 'output.txt', 'src/main.py')"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            },
            "required": ["path", "content"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let content = params.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let sandbox_dir = params.get("_sandbox_dir").and_then(|v| v.as_str()).unwrap_or("/tmp/sandbox/default");

        if path.is_empty() {
            return "Error: 'path' parameter is required".to_string();
        }
        if path.contains("..") {
            return "[TOOL_ERROR] Path traversal not allowed".to_string();
        }
        if content.len() > 102_400 {
            return "[TOOL_ERROR] File too large (max 100KB)".to_string();
        }

        let full_path = std::path::Path::new(sandbox_dir).join(path);
        if !full_path.starts_with(sandbox_dir) {
            return "[TOOL_ERROR] Path outside sandbox".to_string();
        }

        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        match std::fs::write(&full_path, content) {
            Ok(_) => format!("Successfully wrote {} bytes to {}", content.len(), path),
            Err(e) => format!("Error writing file: {e}"),
        }
    }
}

/// File list tool — lists files in the sandbox directory.
pub struct SandboxFileListTool;

#[async_trait]
impl Tool for SandboxFileListTool {
    fn name(&self) -> &str { "file_list" }
    fn description(&self) -> &str {
        "List files and directories in the sandbox. Useful to see what files have been created."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative directory path within the sandbox (default: root of sandbox)"
                }
            }
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let path = params.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let sandbox_dir = params.get("_sandbox_dir").and_then(|v| v.as_str()).unwrap_or("/tmp/sandbox/default");

        if path.contains("..") {
            return "[TOOL_ERROR] Path traversal not allowed".to_string();
        }

        let full_path = std::path::Path::new(sandbox_dir).join(path);
        if !full_path.starts_with(sandbox_dir) {
            return "[TOOL_ERROR] Path outside sandbox".to_string();
        }
        if !full_path.exists() {
            return format!("Directory not found: {path}");
        }

        match std::fs::read_dir(&full_path) {
            Ok(entries) => {
                let mut items: Vec<String> = Vec::new();
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let meta = entry.metadata().ok();
                    let suffix = if meta.as_ref().map(|m| m.is_dir()).unwrap_or(false) {
                        "/"
                    } else {
                        ""
                    };
                    let size = meta.map(|m| m.len()).unwrap_or(0);
                    items.push(format!("{}{} ({}B)", name, suffix, size));
                }
                if items.is_empty() {
                    "(empty directory)".to_string()
                } else {
                    items.sort();
                    items.join("\n")
                }
            }
            Err(e) => format!("Error listing directory: {e}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Phone Call tool — Amazon Connect outbound voice calls
// ---------------------------------------------------------------------------

/// Phone call tool using Amazon Connect for outbound voice calls.
pub struct PhoneCallTool;

#[async_trait]
impl Tool for PhoneCallTool {
    fn name(&self) -> &str { "phone_call" }
    fn description(&self) -> &str {
        "Make and manage phone calls via Amazon Connect. Use this when the user asks to call someone. \
         Actions: 'initiate' to start a call, 'status' to check call status, 'end' to hang up, \
         'list_recent' to show recent calls. Phone numbers must be in E.164 format (e.g., +818012345678). \
         For Japanese numbers, convert 090-xxxx-xxxx to +8190xxxxxxxx."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["initiate", "status", "end", "list_recent"],
                    "description": "Action to perform: initiate (start call), status (check call), end (hang up), list_recent (show history)"
                },
                "phone_number": {
                    "type": "string",
                    "description": "Phone number in E.164 format (e.g., +818012345678). Required for 'initiate'."
                },
                "contact_id": {
                    "type": "string",
                    "description": "Amazon Connect contact ID. Required for 'status' and 'end'."
                },
                "purpose": {
                    "type": "string",
                    "description": "Purpose of the call (used for notes/greeting). Optional."
                }
            },
            "required": ["action"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let action = params.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let phone_number = params.get("phone_number").and_then(|v| v.as_str()).unwrap_or("");
        let contact_id = params.get("contact_id").and_then(|v| v.as_str()).unwrap_or("");
        let purpose = params.get("purpose").and_then(|v| v.as_str()).unwrap_or("");
        let session_key = params.get("_session_key").and_then(|v| v.as_str()).unwrap_or("");
        execute_phone_call(action, phone_number, contact_id, purpose, session_key).await
    }
}

/// Execute a phone call action via Amazon Connect.
async fn execute_phone_call(
    action: &str,
    phone_number: &str,
    contact_id: &str,
    purpose: &str,
    session_key: &str,
) -> String {
    match action {
        "initiate" => execute_phone_initiate(phone_number, purpose, session_key).await,
        "status" => execute_phone_status(contact_id).await,
        "end" => execute_phone_end(contact_id).await,
        "list_recent" => execute_phone_list_recent(session_key).await,
        _ => format!("[TOOL_ERROR] Unknown action '{}'. Use: initiate, status, end, list_recent", action),
    }
}

/// Initiate an outbound voice call.
async fn execute_phone_initiate(phone_number: &str, purpose: &str, session_key: &str) -> String {
    if phone_number.is_empty() {
        return "[TOOL_ERROR] phone_number is required for 'initiate' action".to_string();
    }

    // Validate E.164 format
    if !phone_number.starts_with('+') || phone_number.len() < 8 || phone_number.len() > 16 {
        return "[TOOL_ERROR] phone_number must be in E.164 format (e.g., +818012345678)".to_string();
    }

    let instance_id = std::env::var("CONNECT_INSTANCE_ID").unwrap_or_default();
    let contact_flow_id = std::env::var("CONNECT_CONTACT_FLOW_ID").unwrap_or_default();
    let source_phone = std::env::var("CONNECT_PHONE_NUMBER").unwrap_or_default();

    if instance_id.is_empty() || contact_flow_id.is_empty() || source_phone.is_empty() {
        return "[TOOL_ERROR] Amazon Connect is not configured. Set CONNECT_INSTANCE_ID, CONNECT_CONTACT_FLOW_ID, CONNECT_PHONE_NUMBER.".to_string();
    }

    #[cfg(feature = "dynamodb-backend")]
    {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let client = aws_sdk_connect::Client::new(&config);

        let mut req = client.start_outbound_voice_contact()
            .instance_id(&instance_id)
            .contact_flow_id(&contact_flow_id)
            .destination_phone_number(phone_number)
            .source_phone_number(&source_phone);

        // Add purpose as attribute for the contact flow
        if !purpose.is_empty() {
            req = req.attributes("purpose", purpose);
        }
        if !session_key.is_empty() {
            req = req.attributes("session_key", session_key);
        }

        match req.send().await {
            Ok(output) => {
                let cid = output.contact_id().unwrap_or("unknown");

                // Store call record in DynamoDB
                store_call_record(session_key, cid, phone_number, purpose, "initiated").await;

                serde_json::json!({
                    "status": "initiated",
                    "contact_id": cid,
                    "phone_number": phone_number,
                    "purpose": purpose,
                    "message": format!("Call initiated to {}. Contact ID: {}", phone_number, cid)
                }).to_string()
            }
            Err(e) => format!("[TOOL_ERROR] Failed to initiate call: {e}"),
        }
    }

    #[cfg(not(feature = "dynamodb-backend"))]
    {
        let _ = (purpose, session_key);
        "[TOOL_ERROR] Phone calls require the dynamodb-backend feature".to_string()
    }
}

/// Check the status of an active call.
async fn execute_phone_status(contact_id: &str) -> String {
    if contact_id.is_empty() {
        return "[TOOL_ERROR] contact_id is required for 'status' action".to_string();
    }

    let instance_id = std::env::var("CONNECT_INSTANCE_ID").unwrap_or_default();
    if instance_id.is_empty() {
        return "[TOOL_ERROR] CONNECT_INSTANCE_ID not configured".to_string();
    }

    #[cfg(feature = "dynamodb-backend")]
    {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let client = aws_sdk_connect::Client::new(&config);

        match client
            .describe_contact()
            .instance_id(&instance_id)
            .contact_id(contact_id)
            .send()
            .await
        {
            Ok(output) => {
                if let Some(contact) = output.contact() {
                    let initiation_method = contact.initiation_method()
                        .map(|m| format!("{:?}", m))
                        .unwrap_or_else(|| "unknown".to_string());
                    serde_json::json!({
                        "contact_id": contact_id,
                        "initiation_method": initiation_method,
                        "channel": format!("{:?}", contact.channel().unwrap_or(&aws_sdk_connect::types::Channel::Voice)),
                        "initiation_timestamp": contact.initiation_timestamp().map(|t| t.to_string()),
                        "disconnect_timestamp": contact.disconnect_timestamp().map(|t| t.to_string()),
                    }).to_string()
                } else {
                    format!("{{\"contact_id\": \"{}\", \"status\": \"not_found\"}}", contact_id)
                }
            }
            Err(e) => format!("[TOOL_ERROR] Failed to get contact status: {e}"),
        }
    }

    #[cfg(not(feature = "dynamodb-backend"))]
    {
        "[TOOL_ERROR] Phone calls require the dynamodb-backend feature".to_string()
    }
}

/// End an active call.
async fn execute_phone_end(contact_id: &str) -> String {
    if contact_id.is_empty() {
        return "[TOOL_ERROR] contact_id is required for 'end' action".to_string();
    }

    let instance_id = std::env::var("CONNECT_INSTANCE_ID").unwrap_or_default();
    if instance_id.is_empty() {
        return "[TOOL_ERROR] CONNECT_INSTANCE_ID not configured".to_string();
    }

    #[cfg(feature = "dynamodb-backend")]
    {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let client = aws_sdk_connect::Client::new(&config);

        match client
            .stop_contact()
            .instance_id(&instance_id)
            .contact_id(contact_id)
            .send()
            .await
        {
            Ok(_) => {
                serde_json::json!({
                    "status": "ended",
                    "contact_id": contact_id,
                    "message": "Call has been ended."
                }).to_string()
            }
            Err(e) => format!("[TOOL_ERROR] Failed to end call: {e}"),
        }
    }

    #[cfg(not(feature = "dynamodb-backend"))]
    {
        "[TOOL_ERROR] Phone calls require the dynamodb-backend feature".to_string()
    }
}

/// List recent calls for a user from DynamoDB.
async fn execute_phone_list_recent(session_key: &str) -> String {
    if session_key.is_empty() {
        return "[TOOL_ERROR] Session not available for call history lookup".to_string();
    }

    #[cfg(feature = "dynamodb-backend")]
    {
        use aws_sdk_dynamodb::types::AttributeValue;

        let config_table = std::env::var("DYNAMODB_CONFIG_TABLE").unwrap_or_default();
        if config_table.is_empty() {
            return "[TOOL_ERROR] DynamoDB config table not configured".to_string();
        }

        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let dynamo = aws_sdk_dynamodb::Client::new(&config);

        let pk = format!("CALL#{}", session_key);
        match dynamo
            .query()
            .table_name(&config_table)
            .key_condition_expression("pk = :pk")
            .expression_attribute_values(":pk", AttributeValue::S(pk))
            .scan_index_forward(false)
            .limit(10)
            .send()
            .await
        {
            Ok(output) => {
                let items: Vec<serde_json::Value> = output.items()
                    .iter()
                    .map(|item| {
                        let get_s = |key: &str| item.get(key).and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
                        serde_json::json!({
                            "contact_id": get_s("contact_id"),
                            "phone_number": get_s("phone_number"),
                            "status": get_s("status"),
                            "purpose": get_s("purpose"),
                            "timestamp": get_s("sk"),
                        })
                    })
                    .collect();
                if items.is_empty() {
                    "{\"calls\": [], \"message\": \"No recent calls found.\"}".to_string()
                } else {
                    serde_json::json!({ "calls": items }).to_string()
                }
            }
            Err(e) => format!("[TOOL_ERROR] Failed to query call history: {e}"),
        }
    }

    #[cfg(not(feature = "dynamodb-backend"))]
    {
        "[TOOL_ERROR] Call history requires the dynamodb-backend feature".to_string()
    }
}

/// Store a call record in DynamoDB.
#[cfg(feature = "dynamodb-backend")]
async fn store_call_record(
    session_key: &str,
    contact_id: &str,
    phone_number: &str,
    purpose: &str,
    status: &str,
) {
    use aws_sdk_dynamodb::types::AttributeValue;

    let config_table = std::env::var("DYNAMODB_CONFIG_TABLE").unwrap_or_default();
    if config_table.is_empty() || session_key.is_empty() {
        return;
    }

    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let dynamo = aws_sdk_dynamodb::Client::new(&config);

    let now = chrono::Utc::now();
    let pk = format!("CALL#{}", session_key);
    let sk = format!("{}#{}", now.to_rfc3339(), contact_id);
    let ttl = (now + chrono::Duration::days(365)).timestamp();

    let _ = dynamo.put_item()
        .table_name(&config_table)
        .item("pk", AttributeValue::S(pk))
        .item("sk", AttributeValue::S(sk))
        .item("contact_id", AttributeValue::S(contact_id.to_string()))
        .item("phone_number", AttributeValue::S(phone_number.to_string()))
        .item("status", AttributeValue::S(status.to_string()))
        .item("purpose", AttributeValue::S(purpose.to_string()))
        .item("ttl", AttributeValue::N(ttl.to_string()))
        .send()
        .await;
}

// ---------------------------------------------------------------------------
// Web Deploy Tool — deploy sandbox files to S3 + CloudFront subdomain
// ---------------------------------------------------------------------------

pub struct WebDeployTool;

#[async_trait]
impl Tool for WebDeployTool {
    fn name(&self) -> &str { "web_deploy" }
    fn description(&self) -> &str {
        "Deploy a website from the sandbox to the internet. Uploads files to S3, serves via CloudFront, \
         and assigns a {project}.chatweb.ai subdomain. Use after creating HTML/CSS/JS files with file_write."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "project_name": {
                    "type": "string",
                    "description": "Project name (becomes subdomain: {name}.chatweb.ai). Lowercase alphanumeric + hyphens, 3-30 chars."
                },
                "directory": {
                    "type": "string",
                    "description": "Directory within sandbox to deploy. Use '.' for all files in sandbox root."
                }
            },
            "required": ["project_name", "directory"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let project_name = params.get("project_name").and_then(|v| v.as_str()).unwrap_or("");
        let directory = params.get("directory").and_then(|v| v.as_str()).unwrap_or(".");
        let sandbox_dir = params.get("_sandbox_dir").and_then(|v| v.as_str()).unwrap_or("");
        let session_key = params.get("_session_key").and_then(|v| v.as_str()).unwrap_or("");
        execute_web_deploy(project_name, directory, sandbox_dir, session_key).await
    }
}

/// Validate project name: lowercase alphanumeric + hyphens, 3-30 chars, no leading/trailing hyphen.
fn validate_project_name(name: &str) -> Result<(), String> {
    if name.len() < 3 || name.len() > 30 {
        return Err("[TOOL_ERROR] project_name must be 3-30 characters".to_string());
    }
    if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return Err("[TOOL_ERROR] project_name must contain only lowercase letters, digits, and hyphens".to_string());
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err("[TOOL_ERROR] project_name must not start or end with a hyphen".to_string());
    }
    // Reserved names
    let reserved = ["www", "api", "app", "admin", "mail", "ftp", "cdn", "dev", "staging"];
    if reserved.contains(&name) {
        return Err(format!("[TOOL_ERROR] '{}' is a reserved subdomain name", name));
    }
    Ok(())
}

/// Guess MIME content type from file extension.
#[cfg(feature = "dynamodb-backend")]
fn guess_content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") | Some("mjs") => "application/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ico") => "image/x-icon",
        Some("xml") => "application/xml",
        Some("txt") => "text/plain; charset=utf-8",
        Some("md") => "text/markdown; charset=utf-8",
        Some("pdf") => "application/pdf",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    }
}

async fn execute_web_deploy(
    project_name: &str,
    directory: &str,
    sandbox_dir: &str,
    session_key: &str,
) -> String {
    // Validate inputs
    if sandbox_dir.is_empty() {
        return "[TOOL_ERROR] No sandbox directory. This tool must be used within a chat session.".to_string();
    }
    if let Err(e) = validate_project_name(project_name) {
        return e;
    }

    let base_path = std::path::Path::new(sandbox_dir);
    let deploy_dir = if directory == "." { base_path.to_path_buf() } else { base_path.join(directory) };

    if !deploy_dir.exists() || !deploy_dir.is_dir() {
        return format!("[TOOL_ERROR] Directory '{}' does not exist in sandbox. Create files with file_write first.", directory);
    }

    // Collect files (max 50 files, 10MB total)
    let max_files: usize = 50;
    let max_total_bytes: u64 = 10 * 1024 * 1024;
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut total_bytes: u64 = 0;

    for entry in walkdir::WalkDir::new(&deploy_dir)
        .follow_links(false)
        .max_depth(5)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        // Security: skip hidden files and common unwanted patterns
        let name = entry.file_name().to_string_lossy();
        if name.starts_with('.') || name.ends_with('~') {
            continue;
        }

        if files.len() >= max_files {
            return format!("[TOOL_ERROR] Too many files (max {}). Reduce file count or deploy a subdirectory.", max_files);
        }

        let relative = entry.path().strip_prefix(&deploy_dir)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        match tokio::fs::read(entry.path()).await {
            Ok(content) => {
                total_bytes += content.len() as u64;
                if total_bytes > max_total_bytes {
                    return format!("[TOOL_ERROR] Total size exceeds {}MB limit.", max_total_bytes / 1024 / 1024);
                }
                files.push((relative, content));
            }
            Err(e) => {
                return format!("[TOOL_ERROR] Failed to read file '{}': {}", relative, e);
            }
        }
    }

    if files.is_empty() {
        return "[TOOL_ERROR] No files found to deploy. Create files with file_write first.".to_string();
    }

    // Check for index.html
    let has_index = files.iter().any(|(name, _)| name == "index.html");
    if !has_index {
        return "[TOOL_ERROR] No index.html found. A website needs an index.html entry point.".to_string();
    }

    #[cfg(feature = "dynamodb-backend")]
    {
        let bucket = std::env::var("SITES_S3_BUCKET").unwrap_or_default();
        if bucket.is_empty() {
            return "[TOOL_ERROR] SITES_S3_BUCKET not configured".to_string();
        }

        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let s3 = aws_sdk_s3::Client::new(&config);

        // Upload files to S3
        let mut uploaded: Vec<String> = Vec::new();
        for (relative_path, content) in &files {
            let key = format!("{}/{}", project_name, relative_path);
            let content_type = guess_content_type(relative_path);

            match s3.put_object()
                .bucket(&bucket)
                .key(&key)
                .body(content.clone().into())
                .content_type(content_type)
                .cache_control("public, max-age=3600")
                .send()
                .await
            {
                Ok(_) => uploaded.push(relative_path.clone()),
                Err(e) => return format!("[TOOL_ERROR] S3 upload failed for '{}': {}", relative_path, e),
            }
        }

        // Save site record to DynamoDB
        let config_table = std::env::var("DYNAMODB_CONFIG_TABLE").unwrap_or_default();
        if !config_table.is_empty() {
            use aws_sdk_dynamodb::types::AttributeValue;
            let dynamo = aws_sdk_dynamodb::Client::new(&config);
            let now = chrono::Utc::now();

            let file_list: Vec<AttributeValue> = uploaded.iter()
                .map(|f| AttributeValue::S(f.clone()))
                .collect();

            let _ = dynamo.put_item()
                .table_name(&config_table)
                .item("pk", AttributeValue::S(format!("SITE#{}", project_name)))
                .item("sk", AttributeValue::S("META".to_string()))
                .item("session_key", AttributeValue::S(session_key.to_string()))
                .item("files", AttributeValue::L(file_list))
                .item("file_count", AttributeValue::N(uploaded.len().to_string()))
                .item("total_bytes", AttributeValue::N(total_bytes.to_string()))
                .item("created_at", AttributeValue::S(now.to_rfc3339()))
                .item("updated_at", AttributeValue::S(now.to_rfc3339()))
                .send()
                .await;
        }

        // Ensure Route53 subdomain (if configured)
        let hosted_zone_id = std::env::var("CHATWEB_HOSTED_ZONE_ID").unwrap_or_default();
        let cf_domain = std::env::var("SITES_CF_DOMAIN").unwrap_or_default();
        if !hosted_zone_id.is_empty() && !cf_domain.is_empty() {
            let route53 = aws_sdk_route53::Client::new(&config);
            if let Err(e) = ensure_subdomain(&route53, &hosted_zone_id, project_name, &cf_domain).await {
                tracing::warn!("Route53 subdomain setup failed (site still accessible via CloudFront): {}", e);
            }
        }

        let url = format!("https://{}.chatweb.ai/", project_name);
        let file_list_str = uploaded.iter()
            .map(|f| format!("  - {}", f))
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "✅ Deployed successfully!\n\n🌐 URL: {}\n📁 Files ({}):\n{}\n\nThe site is now live. DNS propagation may take a few minutes for first-time deployments.",
            url, uploaded.len(), file_list_str
        )
    }

    #[cfg(not(feature = "dynamodb-backend"))]
    {
        let _ = (session_key, files, total_bytes);
        "[TOOL_ERROR] Web deploy requires the dynamodb-backend feature (AWS access)".to_string()
    }
}

/// Create or update a Route53 CNAME record for {subdomain}.chatweb.ai.
#[cfg(feature = "dynamodb-backend")]
async fn ensure_subdomain(
    route53: &aws_sdk_route53::Client,
    hosted_zone_id: &str,
    subdomain: &str,
    cloudfront_domain: &str,
) -> Result<(), String> {
    use aws_sdk_route53::types::{
        ChangeBatch, Change, ChangeAction, ResourceRecordSet, RrType, ResourceRecord,
    };

    let record = ResourceRecord::builder()
        .value(cloudfront_domain.to_string())
        .build()
        .map_err(|e| format!("Route53 ResourceRecord build error: {}", e))?;

    let record_set = ResourceRecordSet::builder()
        .name(format!("{}.chatweb.ai", subdomain))
        .set_type(Some(RrType::Cname))
        .ttl(300)
        .resource_records(record)
        .build()
        .map_err(|e| format!("Route53 ResourceRecordSet build error: {}", e))?;

    let change = Change::builder()
        .action(ChangeAction::Upsert)
        .resource_record_set(record_set)
        .build()
        .map_err(|e| format!("Route53 Change build error: {}", e))?;

    let batch = ChangeBatch::builder()
        .changes(change)
        .build()
        .map_err(|e| format!("Route53 ChangeBatch build error: {}", e))?;

    route53.change_resource_record_sets()
        .hosted_zone_id(hosted_zone_id)
        .change_batch(batch)
        .send()
        .await
        .map_err(|e| format!("Route53 error: {}", e))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Media Generation Tools
// ---------------------------------------------------------------------------

/// Image generation tool using OpenAI gpt-image-1 API.
pub struct ImageGenerateTool;

#[async_trait]
impl Tool for ImageGenerateTool {
    fn name(&self) -> &str { "image_generate" }
    fn description(&self) -> &str {
        "Generate an image from a text description using AI (gpt-image-1). Returns a URL to the generated image. Use when the user asks you to draw, create, or generate an image/picture/illustration."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": { "type": "string", "description": "A detailed description of the image to generate (in English for best results)" },
                "quality": { "type": "string", "enum": ["low", "medium", "high"], "description": "Image quality level. low=$0.011, medium=$0.042, high=$0.167 per image. Default: medium" },
                "size": { "type": "string", "enum": ["1024x1024", "1024x1536", "1536x1024"], "description": "Image dimensions. Default: 1024x1024" }
            },
            "required": ["prompt"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let prompt = params.get("prompt").and_then(|v| v.as_str()).unwrap_or("a beautiful landscape");
        let quality = params.get("quality").and_then(|v| v.as_str()).unwrap_or("medium");
        let size = params.get("size").and_then(|v| v.as_str()).unwrap_or("1024x1024");
        execute_image_generate(prompt, quality, size).await
    }
}

async fn execute_image_generate(prompt: &str, quality: &str, size: &str) -> String {
    let api_key = match std::env::var("OPENAI_API_KEY") {
        Ok(k) if !k.is_empty() => k,
        _ => return "[IMAGE_ERROR] OPENAI_API_KEY not configured".to_string(),
    };

    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "model": "gpt-image-1",
        "prompt": prompt,
        "n": 1,
        "size": size,
        "quality": quality,
    });

    match client.post("https://api.openai.com/v1/images/generations")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&body)
        .send()
        .await
    {
        Ok(resp) => {
            if resp.status().is_success() {
                if let Ok(json) = resp.json::<serde_json::Value>().await {
                    if let Some(url) = json.pointer("/data/0/url").and_then(|v| v.as_str()) {
                        return format!("![Generated Image]({})\n\nImage generated successfully (quality: {}, size: {})", url, quality, size);
                    }
                    // Try b64_json format
                    if let Some(b64) = json.pointer("/data/0/b64_json").and_then(|v| v.as_str()) {
                        return format!("![Generated Image](data:image/png;base64,{})\n\nImage generated successfully (quality: {}, size: {})", &b64[..50], quality, size);
                    }
                    format!("[IMAGE_ERROR] Unexpected response format: {}", json)
                } else {
                    "[IMAGE_ERROR] Failed to parse response".to_string()
                }
            } else {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                format!("[IMAGE_ERROR] API returned {}: {}", status, text)
            }
        }
        Err(e) => format!("[IMAGE_ERROR] Request failed: {}", e),
    }
}

/// Music generation tool using Suno API.
pub struct MusicGenerateTool;

#[async_trait]
impl Tool for MusicGenerateTool {
    fn name(&self) -> &str { "music_generate" }
    fn description(&self) -> &str {
        "Generate a song/music from a text description using AI (Suno). Returns a URL to the generated audio. Use when the user asks you to create music, a song, or a melody. Generation takes 30-60 seconds."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": { "type": "string", "description": "Description of the song to generate (e.g., 'upbeat jazz song about coding in Rust')" },
                "style": { "type": "string", "description": "Music style/genre (e.g., 'jazz', 'pop', 'rock', 'electronic', 'classical'). Optional." },
                "instrumental": { "type": "boolean", "description": "If true, generate instrumental only (no vocals). Default: false" }
            },
            "required": ["prompt"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let prompt = params.get("prompt").and_then(|v| v.as_str()).unwrap_or("a happy song");
        let style = params.get("style").and_then(|v| v.as_str()).unwrap_or("");
        let instrumental = params.get("instrumental").and_then(|v| v.as_bool()).unwrap_or(false);
        execute_music_generate(prompt, style, instrumental).await
    }
}

async fn execute_music_generate(prompt: &str, style: &str, instrumental: bool) -> String {
    let api_key = match std::env::var("SUNO_API_KEY") {
        Ok(k) if !k.is_empty() => k,
        _ => return "[MUSIC_ERROR] SUNO_API_KEY not configured. Music generation is not available.".to_string(),
    };
    let api_base = std::env::var("SUNO_API_BASE").unwrap_or_else(|_| "https://apibox.erweima.ai".to_string());

    let full_prompt = if style.is_empty() {
        prompt.to_string()
    } else {
        format!("{} (style: {})", prompt, style)
    };

    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "prompt": full_prompt,
        "customMode": false,
        "instrumental": instrumental,
    });

    // Submit generation request
    let submit_resp = match client.post(format!("{}/api/v1/generate", api_base))
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return format!("[MUSIC_ERROR] Request failed: {}", e),
    };

    if !submit_resp.status().is_success() {
        let text = submit_resp.text().await.unwrap_or_default();
        return format!("[MUSIC_ERROR] API error: {}", text);
    }

    let submit_json: serde_json::Value = match submit_resp.json().await {
        Ok(j) => j,
        Err(e) => return format!("[MUSIC_ERROR] Parse error: {}", e),
    };

    let task_id = match submit_json.pointer("/data/taskId").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => return format!("[MUSIC_ERROR] No taskId in response: {}", submit_json),
    };

    // Poll for completion (max 90 seconds)
    for _ in 0..18 {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;

        let poll_resp = match client.get(format!("{}/api/v1/generate/record?taskId={}", api_base, task_id))
            .header("Authorization", format!("Bearer {}", api_key))
            .send()
            .await
        {
            Ok(r) => r,
            Err(_) => continue,
        };

        if let Ok(poll_json) = poll_resp.json::<serde_json::Value>().await {
            let status = poll_json.pointer("/data/status").and_then(|v| v.as_str()).unwrap_or("");
            if status == "SUCCESS" || status == "FIRST_SUCCESS" {
                if let Some(songs) = poll_json.pointer("/data/response/sunoData").and_then(|v| v.as_array()) {
                    if let Some(song) = songs.first() {
                        let audio_url = song.get("audioUrl").and_then(|v| v.as_str()).unwrap_or("");
                        let title = song.get("title").and_then(|v| v.as_str()).unwrap_or("Generated Song");
                        return format!("[AUDIO:{}]\n\nMusic generated: \"{}\"", audio_url, title);
                    }
                }
                return format!("[MUSIC_ERROR] Completed but no audio URL found: {}", poll_json);
            }
            if status == "FAILED" {
                return format!("[MUSIC_ERROR] Generation failed: {}", poll_json);
            }
        }
    }
    format!("[MUSIC_ERROR] Generation timed out after 90 seconds. Task ID: {}", task_id)
}

/// Video generation tool using Kling API (via fal.ai or direct).
pub struct VideoGenerateTool;

#[async_trait]
impl Tool for VideoGenerateTool {
    fn name(&self) -> &str { "video_generate" }
    fn description(&self) -> &str {
        "Generate a short video from a text description using AI (Kling). Returns a URL to the generated video. Use when the user asks to create a video or animation. Generation takes 1-3 minutes."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": { "type": "string", "description": "Description of the video to generate (e.g., 'a cat playing in the snow, cinematic')" },
                "duration": { "type": "string", "enum": ["5", "10"], "description": "Video duration in seconds. Default: 5" },
                "mode": { "type": "string", "enum": ["standard", "pro"], "description": "Quality mode. standard is faster/cheaper, pro is higher quality. Default: standard" }
            },
            "required": ["prompt"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let prompt = params.get("prompt").and_then(|v| v.as_str()).unwrap_or("a beautiful scene");
        let duration = params.get("duration").and_then(|v| v.as_str()).unwrap_or("5");
        let mode = params.get("mode").and_then(|v| v.as_str()).unwrap_or("standard");
        execute_video_generate(prompt, duration, mode).await
    }
}

async fn execute_video_generate(prompt: &str, duration: &str, mode: &str) -> String {
    let api_key = match std::env::var("KLING_API_KEY") {
        Ok(k) if !k.is_empty() => k,
        _ => return "[VIDEO_ERROR] KLING_API_KEY not configured. Video generation is not available.".to_string(),
    };
    let api_base = std::env::var("KLING_API_BASE").unwrap_or_else(|_| "https://api.klingai.com".to_string());

    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "prompt": prompt,
        "duration": duration,
        "mode": mode,
        "aspect_ratio": "16:9",
    });

    // Submit generation
    let submit_resp = match client.post(format!("{}/v1/videos/text2video", api_base))
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return format!("[VIDEO_ERROR] Request failed: {}", e),
    };

    if !submit_resp.status().is_success() {
        let text = submit_resp.text().await.unwrap_or_default();
        return format!("[VIDEO_ERROR] API error: {}", text);
    }

    let submit_json: serde_json::Value = match submit_resp.json().await {
        Ok(j) => j,
        Err(e) => return format!("[VIDEO_ERROR] Parse error: {}", e),
    };

    let task_id = match submit_json.pointer("/data/task_id").and_then(|v| v.as_str())
        .or_else(|| submit_json.get("task_id").and_then(|v| v.as_str()))
    {
        Some(id) => id.to_string(),
        None => return format!("[VIDEO_ERROR] No task_id in response: {}", submit_json),
    };

    // Poll for completion (max 3 minutes)
    for _ in 0..36 {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;

        let poll_resp = match client.get(format!("{}/v1/videos/text2video/{}", api_base, task_id))
            .header("Authorization", format!("Bearer {}", api_key))
            .send()
            .await
        {
            Ok(r) => r,
            Err(_) => continue,
        };

        if let Ok(poll_json) = poll_resp.json::<serde_json::Value>().await {
            let status = poll_json.pointer("/data/task_status").and_then(|v| v.as_str()).unwrap_or("");
            if status == "succeed" || status == "completed" {
                if let Some(videos) = poll_json.pointer("/data/task_result/videos").and_then(|v| v.as_array()) {
                    if let Some(video) = videos.first() {
                        let video_url = video.get("url").and_then(|v| v.as_str()).unwrap_or("");
                        return format!("[VIDEO:{}]\n\nVideo generated ({}s, {} mode)", video_url, duration, mode);
                    }
                }
                return format!("[VIDEO_ERROR] Completed but no video URL found: {}", poll_json);
            }
            if status == "failed" {
                return format!("[VIDEO_ERROR] Generation failed: {}", poll_json);
            }
        }
    }
    format!("[VIDEO_ERROR] Generation timed out after 3 minutes. Task ID: {}", task_id)
}

// ---------------------------------------------------------------------------
// Webhook / IFTTT Tool
// ---------------------------------------------------------------------------

/// Webhook trigger tool for IFTTT and similar services.
/// Can be used to trigger smart home actions, notifications, etc.
pub struct WebhookTriggerTool;

#[async_trait]
impl Tool for WebhookTriggerTool {
    fn name(&self) -> &str { "webhook_trigger" }
    fn description(&self) -> &str {
        "Trigger a webhook (IFTTT, Zapier, etc.) to perform real-world actions like \
         unlocking doors, controlling smart devices, or sending notifications. \
         Known triggers: 'mita_unlock' = unlock the intercom/door. \
         For IFTTT, provide event_name and optional value1/value2/value3."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "event_name": {
                    "type": "string",
                    "description": "IFTTT event name to trigger (e.g., 'mita_unlock' for door unlock)"
                },
                "url": {
                    "type": "string",
                    "description": "Full webhook URL (if not using IFTTT event_name). Optional."
                },
                "value1": { "type": "string", "description": "Optional value1 parameter" },
                "value2": { "type": "string", "description": "Optional value2 parameter" },
                "value3": { "type": "string", "description": "Optional value3 parameter" }
            },
            "required": ["event_name"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let event_name = params.get("event_name").and_then(|v| v.as_str()).unwrap_or("");
        let custom_url = params.get("url").and_then(|v| v.as_str()).unwrap_or("");
        let value1 = params.get("value1").and_then(|v| v.as_str()).unwrap_or("");
        let value2 = params.get("value2").and_then(|v| v.as_str()).unwrap_or("");
        let value3 = params.get("value3").and_then(|v| v.as_str()).unwrap_or("");

        let ifttt_key = std::env::var("IFTTT_WEBHOOK_KEY").unwrap_or_default();

        let url = if !custom_url.is_empty() {
            custom_url.to_string()
        } else if !ifttt_key.is_empty() && !event_name.is_empty() {
            format!("https://maker.ifttt.com/trigger/{}/with/key/{}", event_name, ifttt_key)
        } else {
            return "[TOOL_ERROR] No IFTTT_WEBHOOK_KEY configured and no custom URL provided.".to_string();
        };

        // Build JSON body
        let mut body = serde_json::Map::new();
        if !value1.is_empty() { body.insert("value1".to_string(), serde_json::json!(value1)); }
        if !value2.is_empty() { body.insert("value2".to_string(), serde_json::json!(value2)); }
        if !value3.is_empty() { body.insert("value3".to_string(), serde_json::json!(value3)); }

        let client = reqwest::Client::new();
        match client.post(&url)
            .header("Content-Type", "application/json")
            .json(&serde_json::Value::Object(body))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
        {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let text = resp.text().await.unwrap_or_default();
                if status >= 200 && status < 300 {
                    format!("Webhook triggered successfully! Event: {}, Status: {}", event_name, status)
                } else {
                    format!("[TOOL_ERROR] Webhook returned status {}: {}", status, text)
                }
            }
            Err(e) => format!("[TOOL_ERROR] Webhook request failed: {}", e),
        }
    }
}

// ─── Slack Integration Tool ───

pub struct SlackTool;

#[async_trait::async_trait]
impl Tool for SlackTool {
    fn name(&self) -> &str { "slack" }
    fn description(&self) -> &str {
        "Read and send Slack messages. Actions: 'send' (post message to channel), \
         'read' (get recent messages from channel), 'search' (search messages). \
         Requires SLACK_BOT_TOKEN env var."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["send", "read", "search"], "description": "Action to perform" },
                "channel": { "type": "string", "description": "Channel name or ID (e.g., '#general' or 'C01234')" },
                "message": { "type": "string", "description": "Message text to send (for 'send' action)" },
                "query": { "type": "string", "description": "Search query (for 'search' action)" },
                "limit": { "type": "integer", "description": "Number of messages to return (default 10)" }
            },
            "required": ["action"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let token = match std::env::var("SLACK_BOT_TOKEN") {
            Ok(t) if !t.is_empty() => t,
            _ => return "[TOOL_ERROR] SLACK_BOT_TOKEN not configured".to_string(),
        };

        let action = params.get("action").and_then(|v| v.as_str()).unwrap_or("read");
        let channel = params.get("channel").and_then(|v| v.as_str()).unwrap_or("");
        let client = reqwest::Client::new();

        match action {
            "send" => {
                let message = params.get("message").and_then(|v| v.as_str()).unwrap_or("");
                if channel.is_empty() || message.is_empty() {
                    return "[TOOL_ERROR] channel and message required for send".to_string();
                }
                let resp = client.post("https://slack.com/api/chat.postMessage")
                    .header("Authorization", format!("Bearer {}", token))
                    .json(&serde_json::json!({ "channel": channel, "text": message }))
                    .send().await;
                match resp {
                    Ok(r) => {
                        let body: serde_json::Value = r.json().await.unwrap_or_default();
                        if body["ok"].as_bool() == Some(true) {
                            format!("Message sent to {}", channel)
                        } else {
                            format!("[TOOL_ERROR] Slack error: {}", body["error"].as_str().unwrap_or("unknown"))
                        }
                    }
                    Err(e) => format!("[TOOL_ERROR] {}", e),
                }
            }
            "read" => {
                if channel.is_empty() {
                    return "[TOOL_ERROR] channel required for read".to_string();
                }
                let limit = params.get("limit").and_then(|v| v.as_i64()).unwrap_or(10);
                let resp = client.get("https://slack.com/api/conversations.history")
                    .header("Authorization", format!("Bearer {}", token))
                    .query(&[("channel", channel), ("limit", &limit.to_string())])
                    .send().await;
                match resp {
                    Ok(r) => {
                        let body: serde_json::Value = r.json().await.unwrap_or_default();
                        if body["ok"].as_bool() == Some(true) {
                            let msgs = body["messages"].as_array().map(|arr| {
                                arr.iter().map(|m| {
                                    let user = m["user"].as_str().unwrap_or("?");
                                    let text = m["text"].as_str().unwrap_or("");
                                    format!("[{}] {}", user, text)
                                }).collect::<Vec<_>>().join("\n")
                            }).unwrap_or_default();
                            if msgs.is_empty() { "No messages found".to_string() } else { msgs }
                        } else {
                            format!("[TOOL_ERROR] {}", body["error"].as_str().unwrap_or("unknown"))
                        }
                    }
                    Err(e) => format!("[TOOL_ERROR] {}", e),
                }
            }
            "search" => {
                let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
                if query.is_empty() {
                    return "[TOOL_ERROR] query required for search".to_string();
                }
                let resp = client.get("https://slack.com/api/search.messages")
                    .header("Authorization", format!("Bearer {}", token))
                    .query(&[("query", query), ("count", "5")])
                    .send().await;
                match resp {
                    Ok(r) => {
                        let body: serde_json::Value = r.json().await.unwrap_or_default();
                        if body["ok"].as_bool() == Some(true) {
                            let matches = body["messages"]["matches"].as_array().map(|arr| {
                                arr.iter().take(5).map(|m| {
                                    let ch = m["channel"]["name"].as_str().unwrap_or("?");
                                    let text = m["text"].as_str().unwrap_or("");
                                    format!("[#{}] {}", ch, if text.len() > 200 { &text[..200] } else { text })
                                }).collect::<Vec<_>>().join("\n---\n")
                            }).unwrap_or_default();
                            if matches.is_empty() { "No results found".to_string() } else { matches }
                        } else {
                            format!("[TOOL_ERROR] {}", body["error"].as_str().unwrap_or("unknown"))
                        }
                    }
                    Err(e) => format!("[TOOL_ERROR] {}", e),
                }
            }
            _ => "[TOOL_ERROR] Unknown action. Use: send, read, search".to_string(),
        }
    }
}

// ─── YouTube Transcript Tool ───

pub struct YouTubeTranscriptTool;

#[async_trait::async_trait]
impl Tool for YouTubeTranscriptTool {
    fn name(&self) -> &str { "youtube_transcript" }
    fn description(&self) -> &str {
        "Fetch the transcript (subtitles/captions) of a YouTube video for summarization or analysis. \
         Provide a YouTube URL or video ID."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "YouTube video URL or video ID (e.g., 'dQw4w9WgXcQ' or 'https://youtu.be/dQw4w9WgXcQ')" },
                "language": { "type": "string", "description": "Preferred language (e.g., 'ja', 'en'). Default: auto" }
            },
            "required": ["url"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let url = params.get("url").and_then(|v| v.as_str()).unwrap_or("");
        if url.is_empty() {
            return "[TOOL_ERROR] url required".to_string();
        }

        // Extract video ID
        let video_id = if url.contains("youtube.com") || url.contains("youtu.be") {
            if url.contains("v=") {
                url.split("v=").nth(1).unwrap_or("").split('&').next().unwrap_or("")
            } else if url.contains("youtu.be/") {
                url.split("youtu.be/").nth(1).unwrap_or("").split('?').next().unwrap_or("")
            } else {
                url
            }
        } else {
            url // assume it's a bare video ID
        };

        if video_id.is_empty() || video_id.len() < 5 {
            return "[TOOL_ERROR] Could not extract valid video ID".to_string();
        }

        let lang = params.get("language").and_then(|v| v.as_str()).unwrap_or("ja");

        // Try fetching transcript via YouTube's timedtext API
        let client = reqwest::Client::new();

        // First get the video page to extract caption tracks
        let page_url = format!("https://www.youtube.com/watch?v={}", video_id);
        let page_resp = client.get(&page_url)
            .header("Accept-Language", format!("{},en;q=0.5", lang))
            .send().await;

        match page_resp {
            Ok(resp) => {
                let body = resp.text().await.unwrap_or_default();
                // Extract captions URL from page source
                if let Some(start) = body.find("\"captions\":") {
                    let captions_json = &body[start..];
                    if let Some(url_start) = captions_json.find("\"baseUrl\":\"") {
                        let url_part = &captions_json[url_start + 11..];
                        if let Some(url_end) = url_part.find('"') {
                            let caption_url = url_part[..url_end]
                                .replace("\\u0026", "&")
                                .replace("\\/", "/");

                            // Add language param
                            let full_url = if caption_url.contains("lang=") {
                                caption_url.to_string()
                            } else {
                                format!("{}&lang={}", caption_url, lang)
                            };

                            match client.get(&full_url).send().await {
                                Ok(r) => {
                                    let xml = r.text().await.unwrap_or_default();
                                    // Parse XML transcript: extract text between <text> tags
                                    let mut transcript = String::new();
                                    for line in xml.split("<text") {
                                        if let Some(content_start) = line.find('>') {
                                            let content = &line[content_start + 1..];
                                            if let Some(end) = content.find("</text>") {
                                                let text = &content[..end]
                                                    .replace("&amp;", "&")
                                                    .replace("&lt;", "<")
                                                    .replace("&gt;", ">")
                                                    .replace("&quot;", "\"")
                                                    .replace("&#39;", "'")
                                                    .replace("\n", " ");
                                                if !text.is_empty() {
                                                    transcript.push_str(&text);
                                                    transcript.push(' ');
                                                }
                                            }
                                        }
                                    }
                                    if transcript.is_empty() {
                                        format!("No transcript found for video {} in language '{}'", video_id, lang)
                                    } else {
                                        // Truncate if too long
                                        if transcript.len() > 15000 {
                                            let mut i = 15000;
                                            while i > 0 && !transcript.is_char_boundary(i) { i -= 1; }
                                            format!("{}... [truncated, {} chars total]", &transcript[..i], transcript.len())
                                        } else {
                                            transcript
                                        }
                                    }
                                }
                                Err(e) => format!("[TOOL_ERROR] Failed to fetch captions: {}", e),
                            }
                        } else {
                            format!("No captions available for video {}", video_id)
                        }
                    } else {
                        format!("No captions available for video {}", video_id)
                    }
                } else {
                    format!("No captions found for video {}. The video may not have subtitles.", video_id)
                }
            }
            Err(e) => format!("[TOOL_ERROR] Failed to fetch video page: {}", e),
        }
    }
}

// ─── Notion Integration Tool ───

pub struct NotionTool;

#[async_trait::async_trait]
impl Tool for NotionTool {
    fn name(&self) -> &str { "notion" }
    fn description(&self) -> &str {
        "Interact with Notion. Actions: 'search' (search pages/databases), \
         'read' (read a page), 'create' (create a new page). \
         Requires NOTION_API_KEY env var."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["search", "read", "create"], "description": "Action to perform" },
                "query": { "type": "string", "description": "Search query (for 'search' action)" },
                "page_id": { "type": "string", "description": "Page ID to read (for 'read' action)" },
                "parent_id": { "type": "string", "description": "Parent page/database ID (for 'create' action)" },
                "title": { "type": "string", "description": "Page title (for 'create' action)" },
                "content": { "type": "string", "description": "Page content in plain text (for 'create' action)" }
            },
            "required": ["action"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let token = match std::env::var("NOTION_API_KEY") {
            Ok(t) if !t.is_empty() => t,
            _ => return "[TOOL_ERROR] NOTION_API_KEY not configured".to_string(),
        };

        let action = params.get("action").and_then(|v| v.as_str()).unwrap_or("search");
        let client = reqwest::Client::new();
        let headers = |c: reqwest::RequestBuilder| -> reqwest::RequestBuilder {
            c.header("Authorization", format!("Bearer {}", token))
             .header("Notion-Version", "2022-06-28")
             .header("Content-Type", "application/json")
        };

        match action {
            "search" => {
                let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
                let resp = headers(client.post("https://api.notion.com/v1/search"))
                    .json(&serde_json::json!({ "query": query, "page_size": 5 }))
                    .send().await;
                match resp {
                    Ok(r) => {
                        let body: serde_json::Value = r.json().await.unwrap_or_default();
                        let results = body["results"].as_array().map(|arr| {
                            arr.iter().take(5).map(|item| {
                                let title = item["properties"]["title"]["title"].as_array()
                                    .and_then(|a| a.first())
                                    .and_then(|t| t["plain_text"].as_str())
                                    .or_else(|| item["properties"]["Name"]["title"].as_array()
                                        .and_then(|a| a.first())
                                        .and_then(|t| t["plain_text"].as_str()))
                                    .unwrap_or("Untitled");
                                let id = item["id"].as_str().unwrap_or("");
                                let obj_type = item["object"].as_str().unwrap_or("page");
                                format!("- [{}] {} (id: {})", obj_type, title, id)
                            }).collect::<Vec<_>>().join("\n")
                        }).unwrap_or_default();
                        if results.is_empty() { "No results found".to_string() } else { results }
                    }
                    Err(e) => format!("[TOOL_ERROR] {}", e),
                }
            }
            "read" => {
                let page_id = params.get("page_id").and_then(|v| v.as_str()).unwrap_or("");
                if page_id.is_empty() {
                    return "[TOOL_ERROR] page_id required for read".to_string();
                }
                let url = format!("https://api.notion.com/v1/blocks/{}/children?page_size=50", page_id);
                let resp = headers(client.get(&url)).send().await;
                match resp {
                    Ok(r) => {
                        let body: serde_json::Value = r.json().await.unwrap_or_default();
                        let blocks = body["results"].as_array().map(|arr| {
                            arr.iter().filter_map(|block| {
                                let btype = block["type"].as_str()?;
                                let rich_text = block[btype]["rich_text"].as_array()?;
                                let text: String = rich_text.iter()
                                    .filter_map(|t| t["plain_text"].as_str())
                                    .collect::<Vec<_>>().join("");
                                if text.is_empty() { None } else { Some(text) }
                            }).collect::<Vec<_>>().join("\n")
                        }).unwrap_or_default();
                        if blocks.is_empty() { "Page is empty or could not read content".to_string() } else { blocks }
                    }
                    Err(e) => format!("[TOOL_ERROR] {}", e),
                }
            }
            "create" => {
                let parent_id = params.get("parent_id").and_then(|v| v.as_str()).unwrap_or("");
                let title = params.get("title").and_then(|v| v.as_str()).unwrap_or("Untitled");
                let content = params.get("content").and_then(|v| v.as_str()).unwrap_or("");
                if parent_id.is_empty() {
                    return "[TOOL_ERROR] parent_id required for create".to_string();
                }

                let mut children = vec![];
                for para in content.split('\n') {
                    if !para.trim().is_empty() {
                        children.push(serde_json::json!({
                            "object": "block",
                            "type": "paragraph",
                            "paragraph": {
                                "rich_text": [{ "type": "text", "text": { "content": para } }]
                            }
                        }));
                    }
                }

                let resp = headers(client.post("https://api.notion.com/v1/pages"))
                    .json(&serde_json::json!({
                        "parent": { "page_id": parent_id },
                        "properties": {
                            "title": { "title": [{ "text": { "content": title } }] }
                        },
                        "children": children
                    }))
                    .send().await;
                match resp {
                    Ok(r) => {
                        let body: serde_json::Value = r.json().await.unwrap_or_default();
                        if let Some(id) = body["id"].as_str() {
                            format!("Page created: {} (id: {})", title, id)
                        } else {
                            format!("[TOOL_ERROR] {}", body["message"].as_str().unwrap_or("Unknown error"))
                        }
                    }
                    Err(e) => format!("[TOOL_ERROR] {}", e),
                }
            }
            _ => "[TOOL_ERROR] Unknown action. Use: search, read, create".to_string(),
        }
    }
}

// ─── arXiv Search Tool ───

pub struct ArxivSearchTool;

#[async_trait::async_trait]
impl Tool for ArxivSearchTool {
    fn name(&self) -> &str { "arxiv_search" }
    fn description(&self) -> &str {
        "Search for academic papers on arXiv. Returns titles, authors, abstracts, and links. \
         Great for AI/ML, physics, math, and CS research."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query (e.g., 'transformer attention mechanism')" },
                "max_results": { "type": "integer", "description": "Number of results (default 5, max 10)" }
            },
            "required": ["query"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
        if query.is_empty() {
            return "[TOOL_ERROR] query required".to_string();
        }
        let max_results = params.get("max_results").and_then(|v| v.as_i64()).unwrap_or(5).min(10);

        let url = format!(
            "http://export.arxiv.org/api/query?search_query=all:{}&start=0&max_results={}&sortBy=submittedDate&sortOrder=descending",
            urlencoding::encode(query), max_results
        );

        let client = reqwest::Client::new();
        match client.get(&url).send().await {
            Ok(resp) => {
                let xml = resp.text().await.unwrap_or_default();
                let mut results = Vec::new();
                for entry in xml.split("<entry>").skip(1) {
                    let title = entry.split("<title>").nth(1)
                        .and_then(|s| s.split("</title>").next())
                        .unwrap_or("").trim().replace('\n', " ");
                    let summary = entry.split("<summary>").nth(1)
                        .and_then(|s| s.split("</summary>").next())
                        .unwrap_or("").trim().replace('\n', " ");
                    let link = entry.split("<id>").nth(1)
                        .and_then(|s| s.split("</id>").next())
                        .unwrap_or("").trim();
                    let authors: Vec<&str> = entry.split("<name>")
                        .skip(1)
                        .filter_map(|s| s.split("</name>").next())
                        .collect();
                    let published = entry.split("<published>").nth(1)
                        .and_then(|s| s.split("</published>").next())
                        .unwrap_or("").trim();

                    let summary_short = if summary.len() > 300 {
                        format!("{}...", &summary[..300])
                    } else {
                        summary.to_string()
                    };

                    results.push(format!(
                        "## {}\nAuthors: {}\nDate: {}\nURL: {}\n{}\n",
                        title,
                        authors.join(", "),
                        &published[..10.min(published.len())],
                        link,
                        summary_short
                    ));
                }

                if results.is_empty() {
                    format!("No papers found for '{}'", query)
                } else {
                    results.join("\n---\n")
                }
            }
            Err(e) => format!("[TOOL_ERROR] arXiv API error: {}", e),
        }
    }
}

// ─── Discord Integration Tool ───

pub struct DiscordTool;

#[async_trait::async_trait]
impl Tool for DiscordTool {
    fn name(&self) -> &str { "discord" }
    fn description(&self) -> &str {
        "Send messages to Discord via webhook. Actions: 'send' (post a message or embed to a Discord channel). \
         Requires DISCORD_WEBHOOK_URL env var."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["send"], "description": "Action to perform" },
                "content": { "type": "string", "description": "Message text to send" },
                "username": { "type": "string", "description": "Override the webhook's default username (optional)" },
                "embed_title": { "type": "string", "description": "Embed title (optional, creates a rich embed)" },
                "embed_description": { "type": "string", "description": "Embed description (optional)" },
                "embed_color": { "type": "integer", "description": "Embed color as decimal integer (optional, e.g. 5814783 for blue)" }
            },
            "required": ["action"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let webhook_url = match std::env::var("DISCORD_WEBHOOK_URL") {
            Ok(u) if !u.is_empty() => u,
            _ => return "[TOOL_ERROR] DISCORD_WEBHOOK_URL not configured".to_string(),
        };

        let action = params.get("action").and_then(|v| v.as_str()).unwrap_or("send");

        match action {
            "send" => {
                let content = params.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let username = params.get("username").and_then(|v| v.as_str());
                let embed_title = params.get("embed_title").and_then(|v| v.as_str());
                let embed_description = params.get("embed_description").and_then(|v| v.as_str());
                let embed_color = params.get("embed_color").and_then(|v| v.as_i64());

                if content.is_empty() && embed_title.is_none() {
                    return "[TOOL_ERROR] content or embed_title required for send".to_string();
                }

                let mut body = serde_json::Map::new();
                if !content.is_empty() {
                    body.insert("content".to_string(), serde_json::json!(content));
                }
                if let Some(name) = username {
                    body.insert("username".to_string(), serde_json::json!(name));
                }

                // Build embed if title is provided
                if let Some(title) = embed_title {
                    let mut embed = serde_json::Map::new();
                    embed.insert("title".to_string(), serde_json::json!(title));
                    if let Some(desc) = embed_description {
                        embed.insert("description".to_string(), serde_json::json!(desc));
                    }
                    if let Some(color) = embed_color {
                        embed.insert("color".to_string(), serde_json::json!(color));
                    }
                    body.insert("embeds".to_string(), serde_json::json!([serde_json::Value::Object(embed)]));
                }

                let client = reqwest::Client::new();
                match client.post(&webhook_url)
                    .header("Content-Type", "application/json")
                    .json(&serde_json::Value::Object(body))
                    .send().await
                {
                    Ok(resp) => {
                        let status = resp.status().as_u16();
                        if status >= 200 && status < 300 {
                            "Message sent to Discord successfully".to_string()
                        } else {
                            let text = resp.text().await.unwrap_or_default();
                            format!("[TOOL_ERROR] Discord returned status {}: {}", status, text)
                        }
                    }
                    Err(e) => format!("[TOOL_ERROR] Discord request failed: {}", e),
                }
            }
            _ => "[TOOL_ERROR] Unknown action. Use: send".to_string(),
        }
    }
}

// ─── Spotify Integration Tool ───

pub struct SpotifyTool;

#[async_trait::async_trait]
impl Tool for SpotifyTool {
    fn name(&self) -> &str { "spotify" }
    fn description(&self) -> &str {
        "Search Spotify for tracks, albums, and artists, or get track details. \
         Actions: 'search' (search by query and type), 'get_track' (get track info by ID). \
         Requires SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET env vars."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["search", "get_track"], "description": "Action to perform" },
                "query": { "type": "string", "description": "Search query (for 'search' action)" },
                "search_type": { "type": "string", "enum": ["track", "album", "artist"], "description": "Type to search for (default: 'track')" },
                "track_id": { "type": "string", "description": "Spotify track ID (for 'get_track' action)" },
                "limit": { "type": "integer", "description": "Number of results (default 5, max 10)" }
            },
            "required": ["action"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let client_id = match std::env::var("SPOTIFY_CLIENT_ID") {
            Ok(v) if !v.is_empty() => v,
            _ => return "[TOOL_ERROR] SPOTIFY_CLIENT_ID not configured".to_string(),
        };
        let client_secret = match std::env::var("SPOTIFY_CLIENT_SECRET") {
            Ok(v) if !v.is_empty() => v,
            _ => return "[TOOL_ERROR] SPOTIFY_CLIENT_SECRET not configured".to_string(),
        };

        // Get access token via client credentials flow
        let client = reqwest::Client::new();
        let token_resp = client.post("https://accounts.spotify.com/api/token")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .basic_auth(&client_id, Some(&client_secret))
            .body("grant_type=client_credentials")
            .send().await;

        let access_token = match token_resp {
            Ok(r) => {
                let body: serde_json::Value = r.json().await.unwrap_or_default();
                match body["access_token"].as_str() {
                    Some(t) => t.to_string(),
                    None => return format!("[TOOL_ERROR] Failed to get Spotify token: {}", body),
                }
            }
            Err(e) => return format!("[TOOL_ERROR] Spotify auth failed: {}", e),
        };

        let action = params.get("action").and_then(|v| v.as_str()).unwrap_or("search");

        match action {
            "search" => {
                let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
                if query.is_empty() {
                    return "[TOOL_ERROR] query required for search".to_string();
                }
                let search_type = params.get("search_type").and_then(|v| v.as_str()).unwrap_or("track");
                let limit = params.get("limit").and_then(|v| v.as_i64()).unwrap_or(5).min(10);

                let url = format!(
                    "https://api.spotify.com/v1/search?q={}&type={}&limit={}",
                    urlencoding::encode(query), search_type, limit
                );

                match client.get(&url)
                    .header("Authorization", format!("Bearer {}", access_token))
                    .send().await
                {
                    Ok(r) => {
                        let body: serde_json::Value = r.json().await.unwrap_or_default();
                        let key = format!("{}s", search_type); // "tracks", "albums", "artists"
                        let items = body[&key]["items"].as_array();

                        match items {
                            Some(arr) if !arr.is_empty() => {
                                let results: Vec<String> = arr.iter().map(|item| {
                                    match search_type {
                                        "track" => {
                                            let name = item["name"].as_str().unwrap_or("?");
                                            let artists: Vec<&str> = item["artists"].as_array()
                                                .map(|a| a.iter().filter_map(|x| x["name"].as_str()).collect())
                                                .unwrap_or_default();
                                            let album = item["album"]["name"].as_str().unwrap_or("?");
                                            let duration_ms = item["duration_ms"].as_i64().unwrap_or(0);
                                            let mins = duration_ms / 60000;
                                            let secs = (duration_ms % 60000) / 1000;
                                            let url = item["external_urls"]["spotify"].as_str().unwrap_or("");
                                            format!("- {} by {} | Album: {} | {}:{:02} | {}", name, artists.join(", "), album, mins, secs, url)
                                        }
                                        "album" => {
                                            let name = item["name"].as_str().unwrap_or("?");
                                            let artists: Vec<&str> = item["artists"].as_array()
                                                .map(|a| a.iter().filter_map(|x| x["name"].as_str()).collect())
                                                .unwrap_or_default();
                                            let total = item["total_tracks"].as_i64().unwrap_or(0);
                                            let date = item["release_date"].as_str().unwrap_or("?");
                                            let url = item["external_urls"]["spotify"].as_str().unwrap_or("");
                                            format!("- {} by {} | {} tracks | Released: {} | {}", name, artists.join(", "), total, date, url)
                                        }
                                        "artist" => {
                                            let name = item["name"].as_str().unwrap_or("?");
                                            let followers = item["followers"]["total"].as_i64().unwrap_or(0);
                                            let genres: Vec<&str> = item["genres"].as_array()
                                                .map(|g| g.iter().filter_map(|x| x.as_str()).collect())
                                                .unwrap_or_default();
                                            let url = item["external_urls"]["spotify"].as_str().unwrap_or("");
                                            format!("- {} | Followers: {} | Genres: {} | {}", name, followers, genres.join(", "), url)
                                        }
                                        _ => format!("- {}", item["name"].as_str().unwrap_or("?")),
                                    }
                                }).collect();
                                results.join("\n")
                            }
                            _ => format!("No {} found for '{}'", search_type, query),
                        }
                    }
                    Err(e) => format!("[TOOL_ERROR] Spotify search failed: {}", e),
                }
            }
            "get_track" => {
                let track_id = params.get("track_id").and_then(|v| v.as_str()).unwrap_or("");
                if track_id.is_empty() {
                    return "[TOOL_ERROR] track_id required for get_track".to_string();
                }

                let url = format!("https://api.spotify.com/v1/tracks/{}", track_id);
                match client.get(&url)
                    .header("Authorization", format!("Bearer {}", access_token))
                    .send().await
                {
                    Ok(r) => {
                        let track: serde_json::Value = r.json().await.unwrap_or_default();
                        if track["error"].is_object() {
                            return format!("[TOOL_ERROR] {}", track["error"]["message"].as_str().unwrap_or("Track not found"));
                        }
                        let name = track["name"].as_str().unwrap_or("?");
                        let artists: Vec<&str> = track["artists"].as_array()
                            .map(|a| a.iter().filter_map(|x| x["name"].as_str()).collect())
                            .unwrap_or_default();
                        let album = track["album"]["name"].as_str().unwrap_or("?");
                        let duration_ms = track["duration_ms"].as_i64().unwrap_or(0);
                        let mins = duration_ms / 60000;
                        let secs = (duration_ms % 60000) / 1000;
                        let popularity = track["popularity"].as_i64().unwrap_or(0);
                        let preview_url = track["preview_url"].as_str().unwrap_or("N/A");
                        let spotify_url = track["external_urls"]["spotify"].as_str().unwrap_or("");
                        let release_date = track["album"]["release_date"].as_str().unwrap_or("?");

                        format!(
                            "Track: {}\nArtists: {}\nAlbum: {}\nDuration: {}:{:02}\nPopularity: {}/100\nRelease: {}\nPreview: {}\nSpotify: {}",
                            name, artists.join(", "), album, mins, secs, popularity, release_date, preview_url, spotify_url
                        )
                    }
                    Err(e) => format!("[TOOL_ERROR] Spotify API error: {}", e),
                }
            }
            _ => "[TOOL_ERROR] Unknown action. Use: search, get_track".to_string(),
        }
    }
}

// ─── PostgreSQL Tool ───

pub struct PostgresTool;

#[async_trait::async_trait]
impl Tool for PostgresTool {
    fn name(&self) -> &str { "postgres" }
    fn description(&self) -> &str {
        "Query a PostgreSQL database (read-only). Actions: 'query' (execute SELECT statements), \
         'describe' (list tables or describe a table's columns). \
         SAFETY: Only SELECT statements are allowed. INSERT/UPDATE/DELETE/DROP are rejected. \
         Requires POSTGRES_URL env var."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["query", "describe"], "description": "Action to perform" },
                "sql": { "type": "string", "description": "SQL SELECT query (for 'query' action). Only SELECT is allowed." },
                "table": { "type": "string", "description": "Table name to describe (for 'describe' action). Omit to list all tables." }
            },
            "required": ["action"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let db_url = match std::env::var("POSTGRES_URL") {
            Ok(u) if !u.is_empty() => u,
            _ => return "[TOOL_ERROR] POSTGRES_URL not configured".to_string(),
        };

        let action = params.get("action").and_then(|v| v.as_str()).unwrap_or("describe");

        // We use reqwest to call a simple HTTP-based PostgreSQL proxy if available,
        // but since we don't have a direct PG driver, we'll use the pg REST pattern.
        // For a Lambda environment, we POST SQL to a REST endpoint or use the DATA API.
        // Here we implement a safety-first approach using a simple HTTP proxy pattern.

        match action {
            "query" => {
                let sql = params.get("sql").and_then(|v| v.as_str()).unwrap_or("");
                if sql.is_empty() {
                    return "[TOOL_ERROR] sql required for query".to_string();
                }

                // Safety check: only allow SELECT statements
                let sql_upper = sql.trim().to_uppercase();
                if !sql_upper.starts_with("SELECT") && !sql_upper.starts_with("WITH") {
                    return "[TOOL_ERROR] Only SELECT (and WITH ... SELECT) statements are allowed. \
                            INSERT, UPDATE, DELETE, DROP, ALTER, CREATE, TRUNCATE are forbidden."
                        .to_string();
                }

                // Reject dangerous keywords even within CTEs
                let dangerous = ["INSERT", "UPDATE", "DELETE", "DROP", "ALTER", "CREATE", "TRUNCATE", "GRANT", "REVOKE"];
                for kw in &dangerous {
                    // Check for the keyword as a standalone word (not part of column names)
                    let pattern = format!(" {} ", kw);
                    let padded = format!(" {} ", sql_upper);
                    if padded.contains(&pattern) || sql_upper.starts_with(&format!("{} ", kw)) {
                        return format!("[TOOL_ERROR] {} statements are not allowed. Read-only access only.", kw);
                    }
                }

                // Execute via a lightweight HTTP request to the database URL
                // If POSTGRES_URL is a REST API endpoint (e.g., PostgREST, Supabase)
                let client = reqwest::Client::new();
                if db_url.starts_with("http") {
                    // REST API mode: POST SQL query
                    match client.post(&db_url)
                        .header("Content-Type", "application/json")
                        .json(&serde_json::json!({ "query": sql }))
                        .timeout(std::time::Duration::from_secs(8))
                        .send().await
                    {
                        Ok(r) => {
                            let status = r.status().as_u16();
                            let body = r.text().await.unwrap_or_default();
                            if status >= 200 && status < 300 {
                                // Truncate if very long
                                if body.len() > 15000 {
                                    format!("{}... [truncated, {} chars total]", &body[..15000], body.len())
                                } else {
                                    body
                                }
                            } else {
                                format!("[TOOL_ERROR] Database returned status {}: {}", status, body)
                            }
                        }
                        Err(e) => format!("[TOOL_ERROR] Database query failed: {}", e),
                    }
                } else {
                    // Connection string mode — not directly supported without a PG driver.
                    // Return helpful message.
                    "[TOOL_ERROR] Direct PostgreSQL connections require a database proxy. \
                     Set POSTGRES_URL to an HTTP endpoint (e.g., PostgREST, Supabase REST) instead."
                        .to_string()
                }
            }
            "describe" => {
                let table = params.get("table").and_then(|v| v.as_str()).unwrap_or("");

                let sql = if table.is_empty() {
                    // List all tables
                    "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public' ORDER BY table_name".to_string()
                } else {
                    // Describe specific table — validate table name to prevent injection
                    if !table.chars().all(|c| c.is_alphanumeric() || c == '_') {
                        return "[TOOL_ERROR] Invalid table name".to_string();
                    }
                    format!(
                        "SELECT column_name, data_type, is_nullable, column_default \
                         FROM information_schema.columns WHERE table_name = '{}' ORDER BY ordinal_position",
                        table
                    )
                };

                // Re-use the query path
                let client = reqwest::Client::new();
                if db_url.starts_with("http") {
                    match client.post(&db_url)
                        .header("Content-Type", "application/json")
                        .json(&serde_json::json!({ "query": sql }))
                        .timeout(std::time::Duration::from_secs(8))
                        .send().await
                    {
                        Ok(r) => {
                            let body = r.text().await.unwrap_or_default();
                            if body.is_empty() || body == "[]" {
                                if table.is_empty() {
                                    "No tables found in public schema".to_string()
                                } else {
                                    format!("Table '{}' not found or has no columns", table)
                                }
                            } else {
                                body
                            }
                        }
                        Err(e) => format!("[TOOL_ERROR] Database describe failed: {}", e),
                    }
                } else {
                    "[TOOL_ERROR] Direct PostgreSQL connections require a database proxy. \
                     Set POSTGRES_URL to an HTTP endpoint (e.g., PostgREST, Supabase REST) instead."
                        .to_string()
                }
            }
            _ => "[TOOL_ERROR] Unknown action. Use: query, describe".to_string(),
        }
    }
}

// ─── CSV Analysis Tool ───

pub struct CsvAnalysisTool;

#[async_trait::async_trait]
impl Tool for CsvAnalysisTool {
    fn name(&self) -> &str { "csv_analysis" }
    fn description(&self) -> &str {
        "Parse and analyze CSV data. Actions: 'summary' (get row count, column names, and sample data), \
         'query' (filter rows or compute simple aggregations). \
         Pass CSV content directly as a string parameter. No external dependencies."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["summary", "query"], "description": "Action to perform" },
                "csv_data": { "type": "string", "description": "CSV content as a string (with header row)" },
                "column": { "type": "string", "description": "Column name for filtering/aggregation (for 'query' action)" },
                "operator": { "type": "string", "enum": ["eq", "ne", "gt", "lt", "gte", "lte", "contains", "sum", "avg", "min", "max", "count"], "description": "Operator for query (comparison or aggregation)" },
                "value": { "type": "string", "description": "Value to compare against (for comparison operators)" }
            },
            "required": ["action", "csv_data"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let action = params.get("action").and_then(|v| v.as_str()).unwrap_or("summary");
        let csv_data = params.get("csv_data").and_then(|v| v.as_str()).unwrap_or("");

        if csv_data.is_empty() {
            return "[TOOL_ERROR] csv_data is required".to_string();
        }

        // Parse CSV lines
        let lines: Vec<&str> = csv_data.lines().collect();
        if lines.is_empty() {
            return "[TOOL_ERROR] CSV data is empty".to_string();
        }

        // Parse header
        let headers: Vec<&str> = lines[0].split(',').map(|s| s.trim().trim_matches('"')).collect();
        let data_lines: Vec<Vec<String>> = lines[1..].iter()
            .filter(|l| !l.trim().is_empty())
            .map(|line| {
                // Simple CSV parsing (handles quoted fields with commas)
                let mut fields = Vec::new();
                let mut current = String::new();
                let mut in_quotes = false;
                for ch in line.chars() {
                    match ch {
                        '"' => in_quotes = !in_quotes,
                        ',' if !in_quotes => {
                            fields.push(current.trim().to_string());
                            current = String::new();
                        }
                        _ => current.push(ch),
                    }
                }
                fields.push(current.trim().to_string());
                fields
            })
            .collect();

        match action {
            "summary" => {
                let row_count = data_lines.len();
                let col_count = headers.len();

                // Detect column types by sampling first few rows
                let mut col_types: Vec<&str> = vec!["unknown"; col_count];
                for (i, _) in headers.iter().enumerate() {
                    let sample_values: Vec<&str> = data_lines.iter()
                        .take(5)
                        .filter_map(|row| row.get(i).map(|s| s.as_str()))
                        .collect();

                    let all_numeric = sample_values.iter().all(|v| v.parse::<f64>().is_ok());
                    let all_integer = sample_values.iter().all(|v| v.parse::<i64>().is_ok());

                    if all_integer && !sample_values.is_empty() {
                        col_types[i] = "integer";
                    } else if all_numeric && !sample_values.is_empty() {
                        col_types[i] = "float";
                    } else {
                        col_types[i] = "string";
                    }
                }

                // Build summary
                let mut result = format!("CSV Summary:\n- Rows: {}\n- Columns: {}\n\nColumns:\n", row_count, col_count);
                for (i, header) in headers.iter().enumerate() {
                    result.push_str(&format!("  - {} ({})\n", header, col_types[i]));
                }

                // Show first 3 rows as sample
                result.push_str("\nSample data (first 3 rows):\n");
                for row in data_lines.iter().take(3) {
                    let pairs: Vec<String> = headers.iter().zip(row.iter())
                        .map(|(h, v)| format!("{}: {}", h, v))
                        .collect();
                    result.push_str(&format!("  {}\n", pairs.join(" | ")));
                }

                result
            }
            "query" => {
                let column = params.get("column").and_then(|v| v.as_str()).unwrap_or("");
                let operator = params.get("operator").and_then(|v| v.as_str()).unwrap_or("eq");
                let value = params.get("value").and_then(|v| v.as_str()).unwrap_or("");

                if column.is_empty() {
                    return "[TOOL_ERROR] column required for query".to_string();
                }

                // Find column index
                let col_idx = match headers.iter().position(|h| h.eq_ignore_ascii_case(column)) {
                    Some(i) => i,
                    None => return format!("[TOOL_ERROR] Column '{}' not found. Available: {}", column, headers.join(", ")),
                };

                // Aggregation operators
                match operator {
                    "sum" | "avg" | "min" | "max" | "count" => {
                        let values: Vec<f64> = data_lines.iter()
                            .filter_map(|row| row.get(col_idx)?.parse::<f64>().ok())
                            .collect();

                        if values.is_empty() && operator != "count" {
                            return format!("No numeric values found in column '{}'", column);
                        }

                        match operator {
                            "sum" => format!("Sum of {}: {}", column, values.iter().sum::<f64>()),
                            "avg" => format!("Average of {}: {:.2}", column, values.iter().sum::<f64>() / values.len() as f64),
                            "min" => format!("Min of {}: {}", column, values.iter().cloned().fold(f64::INFINITY, f64::min)),
                            "max" => format!("Max of {}: {}", column, values.iter().cloned().fold(f64::NEG_INFINITY, f64::max)),
                            "count" => format!("Count of rows: {}", data_lines.len()),
                            _ => unreachable!(),
                        }
                    }
                    // Comparison operators — filter rows
                    _ => {
                        let filtered: Vec<&Vec<String>> = data_lines.iter().filter(|row| {
                            let cell = match row.get(col_idx) {
                                Some(c) => c.as_str(),
                                None => return false,
                            };

                            match operator {
                                "eq" => cell == value,
                                "ne" => cell != value,
                                "contains" => cell.to_lowercase().contains(&value.to_lowercase()),
                                "gt" | "lt" | "gte" | "lte" => {
                                    if let (Ok(a), Ok(b)) = (cell.parse::<f64>(), value.parse::<f64>()) {
                                        match operator {
                                            "gt" => a > b,
                                            "lt" => a < b,
                                            "gte" => a >= b,
                                            "lte" => a <= b,
                                            _ => false,
                                        }
                                    } else {
                                        // String comparison
                                        match operator {
                                            "gt" => cell > value,
                                            "lt" => cell < value,
                                            "gte" => cell >= value,
                                            "lte" => cell <= value,
                                            _ => false,
                                        }
                                    }
                                }
                                _ => false,
                            }
                        }).collect();

                        if filtered.is_empty() {
                            format!("No rows match {} {} '{}'", column, operator, value)
                        } else {
                            let mut result = format!("Found {} matching rows:\n", filtered.len());
                            for row in filtered.iter().take(20) {
                                let pairs: Vec<String> = headers.iter().zip(row.iter())
                                    .map(|(h, v)| format!("{}: {}", h, v))
                                    .collect();
                                result.push_str(&format!("  {}\n", pairs.join(" | ")));
                            }
                            if filtered.len() > 20 {
                                result.push_str(&format!("  ... and {} more rows\n", filtered.len() - 20));
                            }
                            result
                        }
                    }
                }
            }
            _ => "[TOOL_ERROR] Unknown action. Use: summary, query".to_string(),
        }
    }
}

// ─── Extended Filesystem Tool ───

pub struct FilesystemTool;

#[async_trait::async_trait]
impl Tool for FilesystemTool {
    fn name(&self) -> &str { "filesystem" }
    fn description(&self) -> &str {
        "Extended file operations in the sandbox. Actions: 'find' (glob search for files), \
         'grep' (search file contents for a pattern), 'diff' (compare two files). \
         All operations are restricted to the sandbox directory."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["find", "grep", "diff"], "description": "Action to perform" },
                "pattern": { "type": "string", "description": "Glob pattern for 'find' (e.g., '*.py', '**/*.json') or regex pattern for 'grep'" },
                "path": { "type": "string", "description": "Directory to search in for 'find'/'grep', or first file for 'diff'. Relative to sandbox." },
                "path2": { "type": "string", "description": "Second file path for 'diff' action. Relative to sandbox." },
                "max_results": { "type": "integer", "description": "Maximum number of results to return (default 50)" }
            },
            "required": ["action"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let sandbox_dir = params.get("_sandbox_dir").and_then(|v| v.as_str()).unwrap_or("/tmp/sandbox/default");
        let action = params.get("action").and_then(|v| v.as_str()).unwrap_or("find");

        let validate_path = |p: &str| -> Result<std::path::PathBuf, String> {
            if p.contains("..") {
                return Err("[TOOL_ERROR] Path traversal not allowed".to_string());
            }
            let full = std::path::Path::new(sandbox_dir).join(p);
            if !full.starts_with(sandbox_dir) {
                return Err("[TOOL_ERROR] Path outside sandbox".to_string());
            }
            Ok(full)
        };

        match action {
            "find" => {
                let pattern = params.get("pattern").and_then(|v| v.as_str()).unwrap_or("*");
                let path = params.get("path").and_then(|v| v.as_str()).unwrap_or(".");
                let max_results = params.get("max_results").and_then(|v| v.as_i64()).unwrap_or(50) as usize;

                let search_dir = match validate_path(path) {
                    Ok(p) => p,
                    Err(e) => return e,
                };

                if !search_dir.exists() {
                    return format!("Directory not found: {}", path);
                }

                // Recursive directory walk with glob matching
                let mut results = Vec::new();
                let mut stack = vec![search_dir.clone()];

                while let Some(dir) = stack.pop() {
                    if results.len() >= max_results { break; }
                    if let Ok(entries) = std::fs::read_dir(&dir) {
                        for entry in entries.flatten() {
                            let entry_path = entry.path();
                            if entry_path.is_dir() {
                                stack.push(entry_path.clone());
                            }
                            // Simple glob matching
                            let name = entry_path.file_name()
                                .and_then(|n| n.to_str()).unwrap_or("");
                            if glob_matches(pattern, name) {
                                let rel = entry_path.strip_prefix(sandbox_dir)
                                    .map(|p| p.display().to_string())
                                    .unwrap_or_else(|_| entry_path.display().to_string());
                                let suffix = if entry_path.is_dir() { "/" } else { "" };
                                results.push(format!("{}{}", rel, suffix));
                                if results.len() >= max_results { break; }
                            }
                        }
                    }
                }

                if results.is_empty() {
                    format!("No files matching '{}' found in {}", pattern, path)
                } else {
                    format!("Found {} file(s):\n{}", results.len(), results.join("\n"))
                }
            }
            "grep" => {
                let pattern = params.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
                let path = params.get("path").and_then(|v| v.as_str()).unwrap_or(".");
                let max_results = params.get("max_results").and_then(|v| v.as_i64()).unwrap_or(50) as usize;

                if pattern.is_empty() {
                    return "[TOOL_ERROR] pattern required for grep".to_string();
                }

                let search_dir = match validate_path(path) {
                    Ok(p) => p,
                    Err(e) => return e,
                };

                let pattern_lower = pattern.to_lowercase();
                let mut results = Vec::new();
                let mut stack = vec![search_dir.clone()];

                while let Some(dir) = stack.pop() {
                    if results.len() >= max_results { break; }
                    if let Ok(entries) = std::fs::read_dir(&dir) {
                        for entry in entries.flatten() {
                            let entry_path = entry.path();
                            if entry_path.is_dir() {
                                stack.push(entry_path);
                                continue;
                            }
                            // Only search text files (skip binary)
                            if let Ok(content) = std::fs::read_to_string(&entry_path) {
                                let rel = entry_path.strip_prefix(sandbox_dir)
                                    .map(|p| p.display().to_string())
                                    .unwrap_or_else(|_| entry_path.display().to_string());
                                for (i, line) in content.lines().enumerate() {
                                    if line.to_lowercase().contains(&pattern_lower) {
                                        results.push(format!("{}:{}: {}", rel, i + 1, line.chars().take(200).collect::<String>()));
                                        if results.len() >= max_results { break; }
                                    }
                                }
                            }
                        }
                    }
                }

                if results.is_empty() {
                    format!("No matches for '{}' found in {}", pattern, path)
                } else {
                    format!("Found {} match(es):\n{}", results.len(), results.join("\n"))
                }
            }
            "diff" => {
                let path1 = params.get("path").and_then(|v| v.as_str()).unwrap_or("");
                let path2 = params.get("path2").and_then(|v| v.as_str()).unwrap_or("");

                if path1.is_empty() || path2.is_empty() {
                    return "[TOOL_ERROR] path and path2 required for diff".to_string();
                }

                let full1 = match validate_path(path1) {
                    Ok(p) => p,
                    Err(e) => return e,
                };
                let full2 = match validate_path(path2) {
                    Ok(p) => p,
                    Err(e) => return e,
                };

                let content1 = match std::fs::read_to_string(&full1) {
                    Ok(c) => c,
                    Err(e) => return format!("[TOOL_ERROR] Cannot read {}: {}", path1, e),
                };
                let content2 = match std::fs::read_to_string(&full2) {
                    Ok(c) => c,
                    Err(e) => return format!("[TOOL_ERROR] Cannot read {}: {}", path2, e),
                };

                if content1 == content2 {
                    return format!("Files are identical: {} and {}", path1, path2);
                }

                // Simple line-by-line diff
                let lines1: Vec<&str> = content1.lines().collect();
                let lines2: Vec<&str> = content2.lines().collect();
                let mut diffs = Vec::new();

                let max_lines = lines1.len().max(lines2.len());
                for i in 0..max_lines {
                    let l1 = lines1.get(i).copied();
                    let l2 = lines2.get(i).copied();
                    match (l1, l2) {
                        (Some(a), Some(b)) if a != b => {
                            diffs.push(format!("Line {}:\n  - {}\n  + {}", i + 1, a, b));
                        }
                        (Some(a), None) => {
                            diffs.push(format!("Line {}: (only in {})\n  - {}", i + 1, path1, a));
                        }
                        (None, Some(b)) => {
                            diffs.push(format!("Line {}: (only in {})\n  + {}", i + 1, path2, b));
                        }
                        _ => {}
                    }
                }

                if diffs.len() > 50 {
                    format!("Showing first 50 of {} differences:\n{}", diffs.len(), diffs[..50].join("\n"))
                } else {
                    format!("{} difference(s) found:\n{}", diffs.len(), diffs.join("\n"))
                }
            }
            _ => "[TOOL_ERROR] Unknown action. Use: find, grep, diff".to_string(),
        }
    }
}

/// Simple glob matching: supports '*' (any sequence) and '?' (any single char).
fn glob_matches(pattern: &str, name: &str) -> bool {
    let pattern_chars: Vec<char> = pattern.chars().collect();
    let name_chars: Vec<char> = name.chars().collect();
    glob_matches_inner(&pattern_chars, &name_chars)
}

fn glob_matches_inner(pattern: &[char], name: &[char]) -> bool {
    match (pattern.first(), name.first()) {
        (None, None) => true,
        (Some('*'), _) => {
            // '*' matches zero or more characters
            glob_matches_inner(&pattern[1..], name) ||
            (!name.is_empty() && glob_matches_inner(pattern, &name[1..]))
        }
        (Some('?'), Some(_)) => glob_matches_inner(&pattern[1..], &name[1..]),
        (Some(pc), Some(nc)) if pc == nc => glob_matches_inner(&pattern[1..], &name[1..]),
        _ => false,
    }
}

// ─── Browser Tool ───

pub struct BrowserTool;

#[async_trait::async_trait]
impl Tool for BrowserTool {
    fn name(&self) -> &str { "browser" }
    fn description(&self) -> &str {
        "Fetch web pages and extract content using CSS selectors. \
         Actions: 'extract' (fetch URL and extract elements matching a CSS selector), \
         'screenshot' (get a screenshot URL via an external service), \
         'fill_form' (simulate form submission via POST). \
         This is a simplified browser that uses HTTP requests (no JavaScript rendering)."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["extract", "screenshot", "fill_form"], "description": "Action to perform" },
                "url": { "type": "string", "description": "URL to fetch or screenshot" },
                "selector": { "type": "string", "description": "CSS selector to extract elements (for 'extract' action, e.g., 'h1', '.title', '#content', 'a[href]')" },
                "attribute": { "type": "string", "description": "Extract specific attribute instead of text content (e.g., 'href', 'src')" },
                "form_data": { "type": "object", "description": "Form data as key-value pairs (for 'fill_form' action)" },
                "method": { "type": "string", "enum": ["GET", "POST"], "description": "HTTP method for fill_form (default: POST)" }
            },
            "required": ["action", "url"]
        })
    }
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let action = params.get("action").and_then(|v| v.as_str()).unwrap_or("extract");
        let url = params.get("url").and_then(|v| v.as_str()).unwrap_or("");

        if url.is_empty() {
            return "[TOOL_ERROR] url is required".to_string();
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .user_agent("Mozilla/5.0 (compatible; ChatWebBot/1.0)")
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        match action {
            "extract" => {
                let selector = params.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                let attribute = params.get("attribute").and_then(|v| v.as_str());

                let html = match client.get(url).send().await {
                    Ok(r) => r.text().await.unwrap_or_default(),
                    Err(e) => return format!("[TOOL_ERROR] Failed to fetch {}: {}", url, e),
                };

                if selector.is_empty() {
                    // Return cleaned text content (strip HTML tags)
                    let text = strip_html_tags(&html);
                    if text.len() > 10000 {
                        let mut i = 10000;
                        while i > 0 && !text.is_char_boundary(i) { i -= 1; }
                        format!("{}... [truncated, {} chars total]", &text[..i], text.len())
                    } else {
                        text
                    }
                } else {
                    // Simple CSS selector extraction
                    // Supports: tag, .class, #id, tag.class, tag[attr]
                    let elements = extract_by_selector(&html, selector, attribute);
                    if elements.is_empty() {
                        format!("No elements matching '{}' found on {}", selector, url)
                    } else {
                        let count = elements.len();
                        let display: Vec<String> = elements.into_iter().take(50).collect();
                        if count > 50 {
                            format!("Found {} elements (showing first 50):\n{}", count, display.join("\n"))
                        } else {
                            format!("Found {} element(s):\n{}", count, display.join("\n"))
                        }
                    }
                }
            }
            "screenshot" => {
                // Use a free screenshot service
                let encoded_url = urlencoding::encode(url);
                let screenshot_url = format!(
                    "https://api.screenshotmachine.com?key=guest&url={}&dimension=1024x768",
                    encoded_url
                );
                format!("Screenshot URL: {}\n\nNote: This uses a third-party screenshot service. The page is rendered server-side.", screenshot_url)
            }
            "fill_form" => {
                let form_data = params.get("form_data").and_then(|v| v.as_object());
                let method = params.get("method").and_then(|v| v.as_str()).unwrap_or("POST");

                let data: HashMap<String, String> = form_data
                    .map(|m| {
                        m.iter()
                            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                            .collect()
                    })
                    .unwrap_or_default();

                if data.is_empty() {
                    return "[TOOL_ERROR] form_data is required for fill_form".to_string();
                }

                let resp = if method == "GET" {
                    client.get(url).query(&data).send().await
                } else {
                    client.post(url).form(&data).send().await
                };

                match resp {
                    Ok(r) => {
                        let status = r.status().as_u16();
                        let body = r.text().await.unwrap_or_default();
                        let text = strip_html_tags(&body);
                        let truncated = if text.len() > 5000 {
                            let mut i = 5000;
                            while i > 0 && !text.is_char_boundary(i) { i -= 1; }
                            format!("{}...", &text[..i])
                        } else {
                            text
                        };
                        format!("Form submitted (status {})\n\nResponse:\n{}", status, truncated)
                    }
                    Err(e) => format!("[TOOL_ERROR] Form submission failed: {}", e),
                }
            }
            _ => "[TOOL_ERROR] Unknown action. Use: extract, screenshot, fill_form".to_string(),
        }
    }
}

/// Simple CSS selector extraction from HTML.
/// Supports: tag name, .class, #id, and tag[attr] patterns.
fn extract_by_selector(html: &str, selector: &str, attribute: Option<&str>) -> Vec<String> {
    let mut results = Vec::new();

    // Determine what we're looking for
    let (tag_filter, class_filter, id_filter) = if selector.starts_with('#') {
        (None, None, Some(&selector[1..]))
    } else if selector.starts_with('.') {
        (None, Some(&selector[1..]), None)
    } else if selector.contains('.') {
        let parts: Vec<&str> = selector.splitn(2, '.').collect();
        (Some(parts[0]), Some(parts[1]), None)
    } else {
        (Some(selector), None, None)
    };

    // Simple tag extraction using string scanning
    // Find all opening tags
    let mut pos = 0;
    while pos < html.len() {
        if let Some(tag_start) = html[pos..].find('<') {
            let abs_start = pos + tag_start;
            if abs_start + 1 >= html.len() || html.as_bytes()[abs_start + 1] == b'/' {
                pos = abs_start + 1;
                continue;
            }

            if let Some(tag_end) = html[abs_start..].find('>') {
                let tag_str = &html[abs_start..abs_start + tag_end + 1];
                let tag_inner = &tag_str[1..tag_str.len() - 1];

                // Get tag name
                let tag_name = tag_inner.split_whitespace().next().unwrap_or("");
                if tag_name.is_empty() || tag_name.starts_with('!') || tag_name.starts_with('?') {
                    pos = abs_start + tag_end + 1;
                    continue;
                }

                let mut matches = true;

                // Check tag name filter
                if let Some(tf) = tag_filter {
                    // Handle tag[attr] pattern
                    let clean_tag = tf.split('[').next().unwrap_or(tf);
                    if !clean_tag.is_empty() && !tag_name.eq_ignore_ascii_case(clean_tag) {
                        matches = false;
                    }
                }

                // Check class filter
                if matches {
                    if let Some(cf) = class_filter {
                        let has_class = tag_inner.contains(&format!("class=\"{}", cf))
                            || tag_inner.contains(&format!("class='{}", cf))
                            || tag_inner.contains(&format!(" {}", cf));
                        if !has_class {
                            matches = false;
                        }
                    }
                }

                // Check id filter
                if matches {
                    if let Some(idf) = id_filter {
                        let has_id = tag_inner.contains(&format!("id=\"{}\"", idf))
                            || tag_inner.contains(&format!("id='{}'", idf));
                        if !has_id {
                            matches = false;
                        }
                    }
                }

                if matches {
                    // Extract content or attribute
                    if let Some(attr) = attribute {
                        // Extract attribute value from the tag
                        let attr_pattern1 = format!("{}=\"", attr);
                        let attr_pattern2 = format!("{}='", attr);
                        if let Some(attr_pos) = tag_inner.find(&attr_pattern1) {
                            let val_start = attr_pos + attr_pattern1.len();
                            if let Some(val_end) = tag_inner[val_start..].find('"') {
                                results.push(tag_inner[val_start..val_start + val_end].to_string());
                            }
                        } else if let Some(attr_pos) = tag_inner.find(&attr_pattern2) {
                            let val_start = attr_pos + attr_pattern2.len();
                            if let Some(val_end) = tag_inner[val_start..].find('\'') {
                                results.push(tag_inner[val_start..val_start + val_end].to_string());
                            }
                        }
                    } else {
                        // Extract text content between opening and closing tags
                        let close_tag = format!("</{}", tag_name);
                        let content_start = abs_start + tag_end + 1;
                        if let Some(close_pos) = html[content_start..].to_lowercase().find(&close_tag.to_lowercase()) {
                            let inner_html = &html[content_start..content_start + close_pos];
                            let text = strip_html_tags(inner_html).trim().to_string();
                            if !text.is_empty() {
                                results.push(text);
                            }
                        }
                    }
                }

                pos = abs_start + tag_end + 1;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculator() {
        assert_eq!(eval_simple_expr("2+3"), Some(5.0));
        assert_eq!(eval_simple_expr("10*5"), Some(50.0));
        assert_eq!(eval_simple_expr("100/4"), Some(25.0));
        assert_eq!(eval_simple_expr("42"), Some(42.0));
    }

    #[test]
    fn test_strip_html() {
        assert_eq!(strip_html_tags("<b>hello</b>"), "hello");
        assert_eq!(strip_html_tags("<div>a b</div>"), "a b");
    }

    #[test]
    fn test_tool_definitions() {
        let tools = get_tool_definitions();
        assert!(tools.len() >= 4);
        let names: Vec<&str> = tools.iter()
            .filter_map(|t| t.pointer("/function/name").and_then(|v| v.as_str()))
            .collect();
        assert!(names.contains(&"web_search"));
        assert!(names.contains(&"calculator"));
        assert!(names.contains(&"weather"));
    }

    #[test]
    fn test_list_integrations() {
        let integrations = list_integrations();
        assert!(integrations.len() >= 4);
        let enabled: Vec<&str> = integrations.iter()
            .filter(|i| i.enabled)
            .map(|i| i.name.as_str())
            .collect();
        assert!(enabled.contains(&"Web Search"));
        assert!(enabled.contains(&"Calculator"));
    }

    #[test]
    fn test_tool_registry_builtins() {
        // Clear env vars for deterministic count
        std::env::remove_var("GITHUB_TOKEN");
        std::env::remove_var("CONNECT_INSTANCE_ID");
        std::env::remove_var("SLACK_BOT_TOKEN");
        std::env::remove_var("NOTION_API_KEY");
        std::env::remove_var("DISCORD_WEBHOOK_URL");
        std::env::remove_var("SPOTIFY_CLIENT_ID");
        std::env::remove_var("POSTGRES_URL");
        let registry = ToolRegistry::with_builtins();
        // 27 base (19 original + 3 always-on: csv_analysis, filesystem, browser + 5 new: git_status, git_diff, git_commit, run_linter, run_tests)
        // + 2 http-api only (youtube_transcript, arxiv_search) + 1 github_read_file = 30 when http-api
        let expected = if cfg!(feature = "http-api") { 30 } else { 27 };
        assert_eq!(registry.len(), expected);
        let defs = registry.get_definitions();
        let names: Vec<&str> = defs.iter()
            .filter_map(|t| t.pointer("/function/name").and_then(|v| v.as_str()))
            .collect();
        assert!(names.contains(&"web_search"));
        assert!(names.contains(&"web_fetch"));
        assert!(names.contains(&"calculator"));
        assert!(names.contains(&"weather"));
        assert!(names.contains(&"translate"));
        assert!(names.contains(&"wikipedia"));
        assert!(names.contains(&"datetime"));
        assert!(names.contains(&"qr_code"));
        assert!(names.contains(&"news_search"));
        // Sandbox tools
        assert!(names.contains(&"code_execute"));
        assert!(names.contains(&"file_read"));
        assert!(names.contains(&"file_write"));
        assert!(names.contains(&"file_list"));
        // http-api only integration tools
        #[cfg(feature = "http-api")]
        {
            assert!(names.contains(&"youtube_transcript"));
            assert!(names.contains(&"arxiv_search"));
        }
        // Always-on new tools
        assert!(names.contains(&"csv_analysis"));
        assert!(names.contains(&"filesystem"));
        assert!(names.contains(&"browser"));
    }

    #[test]
    #[cfg(feature = "http-api")]
    fn test_tool_registry_with_github_token() {
        std::env::set_var("GITHUB_TOKEN", "test-token");
        std::env::remove_var("CONNECT_INSTANCE_ID");
        std::env::remove_var("SLACK_BOT_TOKEN");
        std::env::remove_var("NOTION_API_KEY");
        std::env::remove_var("DISCORD_WEBHOOK_URL");
        std::env::remove_var("SPOTIFY_CLIENT_ID");
        std::env::remove_var("POSTGRES_URL");
        let registry = ToolRegistry::with_builtins();
        assert_eq!(registry.len(), 27); // 24 base + 1 github_read_file + 2 github_write tools
        let defs = registry.get_definitions();
        let names: Vec<&str> = defs.iter()
            .filter_map(|t| t.pointer("/function/name").and_then(|v| v.as_str()))
            .collect();
        assert!(names.contains(&"github_read_file"));
        assert!(names.contains(&"github_create_or_update_file"));
        assert!(names.contains(&"github_create_pr"));
        // Clean up
        std::env::remove_var("GITHUB_TOKEN");
    }

    #[test]
    #[cfg(feature = "http-api")]
    fn test_github_tools_reject_main_branch() {
        // Verify the safety constraint is documented in parameters
        let tool = GitHubCreateOrUpdateFileTool;
        let params = tool.parameters();
        let branch_desc = params.pointer("/properties/branch/description")
            .and_then(|v| v.as_str()).unwrap_or("");
        assert!(branch_desc.contains("main") || branch_desc.contains("master"),
            "Branch parameter should warn about main/master restriction");
    }

    #[test]
    fn test_tool_registry_includes_legacy() {
        let legacy = get_tool_definitions();
        let registry = ToolRegistry::with_builtins();
        let new = registry.get_definitions();
        // Registry has at least as many tools as legacy
        assert!(new.len() >= legacy.len());
        // All legacy tools should be in registry
        for l in &legacy {
            let name = l.pointer("/function/name").and_then(|v| v.as_str()).unwrap();
            assert!(new.iter().any(|n| n.pointer("/function/name").and_then(|v| v.as_str()) == Some(name)),
                "Legacy tool '{name}' not found in registry");
        }
    }

    #[tokio::test]
    async fn test_tool_registry_execute() {
        let registry = ToolRegistry::with_builtins();
        let mut params = HashMap::new();
        params.insert("expression".to_string(), serde_json::json!("2+3"));
        let result = registry.execute("calculator", &params).await;
        assert!(result.contains("5"));
    }

    #[tokio::test]
    async fn test_tool_registry_unknown() {
        let registry = ToolRegistry::with_builtins();
        let params = HashMap::new();
        let result = registry.execute("nonexistent", &params).await;
        assert!(result.contains("Unknown tool"));
    }
}

// ---------------------------------------------------------------------------
// Git Operations Tools
// ---------------------------------------------------------------------------

struct GitStatusTool;

#[async_trait]
impl Tool for GitStatusTool {
    fn name(&self) -> &str {
        "git_status"
    }

    fn description(&self) -> &str {
        "Get git repository status - shows modified, staged, and untracked files. Essential for understanding what changes exist before committing."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Optional: Path to git repository (defaults to current directory)"
                }
            }
        })
    }

    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(".");

        match tokio::process::Command::new("git")
            .args(&["status", "--short"])
            .current_dir(path)
            .output()
            .await
        {
            Ok(output) => {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    if stdout.trim().is_empty() {
                        "✅ Working directory clean - no changes".to_string()
                    } else {
                        format!("Git status:\n{}", stdout)
                    }
                } else {
                    format!("Error: {}", String::from_utf8_lossy(&output.stderr))
                }
            }
            Err(e) => format!("Failed to run git status: {}", e),
        }
    }
}

struct GitDiffTool;

#[async_trait]
impl Tool for GitDiffTool {
    fn name(&self) -> &str {
        "git_diff"
    }

    fn description(&self) -> &str {
        "Show git diff - displays changes in files. Use this to review what will be committed. Can show staged changes (--staged) or unstaged changes (default)."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Optional: Path to git repository or specific file"
                },
                "staged": {
                    "type": "boolean",
                    "description": "Show staged changes (--staged). Default: false (shows unstaged changes)"
                },
                "file": {
                    "type": "string",
                    "description": "Optional: Specific file to diff"
                }
            }
        })
    }

    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(".");
        let staged = params
            .get("staged")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let file = params.get("file").and_then(|v| v.as_str());

        let mut cmd = tokio::process::Command::new("git");
        cmd.arg("diff");

        if staged {
            cmd.arg("--staged");
        }

        if let Some(f) = file {
            cmd.arg(f);
        }

        cmd.current_dir(path);

        match cmd.output().await {
            Ok(output) => {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    if stdout.trim().is_empty() {
                        "No changes to show".to_string()
                    } else {
                        format!("Git diff:\n{}", stdout)
                    }
                } else {
                    format!("Error: {}", String::from_utf8_lossy(&output.stderr))
                }
            }
            Err(e) => format!("Failed to run git diff: {}", e),
        }
    }
}

struct GitCommitTool;

#[async_trait]
impl Tool for GitCommitTool {
    fn name(&self) -> &str {
        "git_commit"
    }

    fn description(&self) -> &str {
        "Create a git commit with a message. Files must be staged first (use git add). This saves your changes to version control history."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "Commit message describing the changes"
                },
                "path": {
                    "type": "string",
                    "description": "Optional: Path to git repository"
                }
            },
            "required": ["message"]
        })
    }

    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let message = match params.get("message").and_then(|v| v.as_str()) {
            Some(m) => m,
            None => return "Error: commit message is required".to_string(),
        };

        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(".");

        match tokio::process::Command::new("git")
            .args(&["commit", "-m", message])
            .current_dir(path)
            .output()
            .await
        {
            Ok(output) => {
                if output.status.success() {
                    format!(
                        "✅ Commit created:\n{}",
                        String::from_utf8_lossy(&output.stdout)
                    )
                } else {
                    format!("Error: {}", String::from_utf8_lossy(&output.stderr))
                }
            }
            Err(e) => format!("Failed to run git commit: {}", e),
        }
    }
}

// ---------------------------------------------------------------------------
// Quality Assurance Tools
// ---------------------------------------------------------------------------

struct RunLinterTool;

#[async_trait]
impl Tool for RunLinterTool {
    fn name(&self) -> &str {
        "run_linter"
    }

    fn description(&self) -> &str {
        "Run code linter to detect syntax errors, style issues, and potential bugs. Supports multiple languages (clippy for Rust, eslint for JS/TS, pylint for Python, etc.). Returns list of issues found."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "language": {
                    "type": "string",
                    "enum": ["rust", "javascript", "typescript", "python", "go"],
                    "description": "Programming language to lint"
                },
                "path": {
                    "type": "string",
                    "description": "Optional: Path to file or directory to lint (defaults to current directory)"
                },
                "fix": {
                    "type": "boolean",
                    "description": "Auto-fix issues if possible (default: false)"
                }
            },
            "required": ["language"]
        })
    }

    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let language = match params.get("language").and_then(|v| v.as_str()) {
            Some(l) => l,
            None => return "Error: language is required".to_string(),
        };

        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(".");
        let fix = params
            .get("fix")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let (cmd, args) = match language {
            "rust" => {
                let mut args = vec!["clippy", "--", "-D", "warnings"];
                if fix {
                    args = vec!["clippy", "--fix", "--allow-dirty"];
                }
                ("cargo", args)
            }
            "javascript" | "typescript" => {
                let mut args = vec!["eslint", path];
                if fix {
                    args.push("--fix");
                }
                ("npx", args)
            }
            "python" => ("pylint", vec![path]),
            "go" => ("golint", vec![path]),
            _ => return format!("Unsupported language: {}", language),
        };

        match tokio::process::Command::new(cmd)
            .args(&args)
            .output()
            .await
        {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let combined = format!("{}\n{}", stdout, stderr);

                if output.status.success() {
                    format!("✅ Linter passed - no issues found\n{}", combined)
                } else {
                    format!("⚠️ Linter found issues:\n{}", combined)
                }
            }
            Err(e) => format!("Failed to run linter (is {} installed?): {}", cmd, e),
        }
    }
}

struct RunTestsTool;

#[async_trait]
impl Tool for RunTestsTool {
    fn name(&self) -> &str {
        "run_tests"
    }

    fn description(&self) -> &str {
        "Run test suite to verify code correctness. Supports multiple test frameworks (cargo test for Rust, jest for JS/TS, pytest for Python, etc.). Returns test results with pass/fail status."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "language": {
                    "type": "string",
                    "enum": ["rust", "javascript", "typescript", "python", "go"],
                    "description": "Programming language / test framework"
                },
                "path": {
                    "type": "string",
                    "description": "Optional: Path to test file or directory"
                },
                "test_name": {
                    "type": "string",
                    "description": "Optional: Specific test name to run (default: run all tests)"
                }
            },
            "required": ["language"]
        })
    }

    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let language = match params.get("language").and_then(|v| v.as_str()) {
            Some(l) => l,
            None => return "Error: language is required".to_string(),
        };

        let path = params.get("path").and_then(|v| v.as_str());
        let test_name = params.get("test_name").and_then(|v| v.as_str());

        let (cmd, mut args) = match language {
            "rust" => ("cargo", vec!["test"]),
            "javascript" | "typescript" => ("npm", vec!["test"]),
            "python" => ("pytest", vec!["-v"]),
            "go" => ("go", vec!["test", "-v"]),
            _ => return format!("Unsupported language: {}", language),
        };

        if let Some(p) = path {
            args.push(p);
        }

        if let Some(t) = test_name {
            if language == "rust" {
                args.push(t);
            }
        }

        match tokio::process::Command::new(cmd)
            .args(&args)
            .output()
            .await
        {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let combined = format!("{}\n{}", stdout, stderr);

                if output.status.success() {
                    format!("✅ Tests passed\n{}", combined)
                } else {
                    format!("❌ Tests failed\n{}", combined)
                }
            }
            Err(e) => format!("Failed to run tests (is {} installed?): {}", cmd, e),
        }
    }
}
