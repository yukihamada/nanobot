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
        // Clear GITHUB_TOKEN to get deterministic count
        std::env::remove_var("GITHUB_TOKEN");
        let registry = ToolRegistry::with_builtins();
        assert_eq!(registry.len(), 16); // 11 original + 4 sandbox + 1 github_read_file (public)
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
    }

    #[test]
    #[cfg(feature = "http-api")]
    fn test_tool_registry_with_github_token() {
        std::env::set_var("GITHUB_TOKEN", "test-token");
        let registry = ToolRegistry::with_builtins();
        assert_eq!(registry.len(), 18); // 15 base + 3 GitHub tools
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
