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
        let tools: Vec<Box<dyn Tool>> = vec![
            Box::new(WebSearchTool),
            Box::new(WebFetchTool),
            Box::new(CalculatorTool),
            Box::new(WeatherTool),
        ];
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

    /// Execute a tool by name.
    pub async fn execute(&self, name: &str, arguments: &HashMap<String, serde_json::Value>) -> String {
        for tool in &self.tools {
            if tool.name() == name {
                return tool.execute(arguments.clone()).await;
            }
        }
        format!("Unknown tool: {}", name)
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

/// Available integration types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationType {
    WebSearch,
    WebFetch,
    Weather,
    Calculator,
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
        _ => format!("Unknown tool: {}", name),
    }
}

/// Web search: try Brave API → Bing HTML → Jina search fallback.
async fn execute_web_search(query: &str) -> String {
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
                        link = format!("https://{}", link);
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
    let jina_url = format!("https://r.jina.ai/{}", ddg_url);

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
                return format!("Search temporarily unavailable (HTTP {}). Try asking in a different way.", status);
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
                        return format!("Search results for \"{}\":\n\n{}", query, snippet);
                    }
                    tracing::warn!("jina_search: response too small ({} useful chars)", useful.len());
                    format!("No results found for \"{}\". Try a more specific query.", query)
                }
                Err(e) => {
                    tracing::warn!("jina_search: body read error: {}", e);
                    format!("Search error: {}", e)
                }
            }
        }
        Err(e) => {
            tracing::warn!("jina_search: request failed: {}", e);
            format!("Search unavailable: {}", e)
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
            && !w.parse::<u32>().map(|n| n >= 2020 && n <= 2030).unwrap_or(false)
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
    let jina_url = format!("https://r.jina.ai/{}", search_url);

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
        let product_jina = format!("https://r.jina.ai/{}", purl);
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
                        return format!("Product details from kakaku.com for \"{}\":\nURL: {}\n\n{}", query, purl, snippet);
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
            return format!("Search results from kakaku.com for \"{}\":\n\n{}", query, snippet);
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
async fn execute_web_fetch(url: &str) -> String {
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
    let jina_url = format!("https://r.jina.ai/{}", url);
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
                            return format!("Content from {}:\n\n{}", url, snippet);
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
                        format!("Content from {}:\n\n{}", url, cleaned)
                    }
                }
                Err(e) => format!("Failed to read page: {}", e),
            }
        }
        Err(e) => format!("Failed to fetch URL: {}", e),
    }
}

/// Simple calculator using basic expression parsing.
fn execute_calculator(expression: &str) -> String {
    // Simple expression evaluator for basic arithmetic
    let expr = expression.replace(' ', "");

    // Try to evaluate as a simple expression
    match eval_simple_expr(&expr) {
        Some(result) => format!("{} = {}", expression, result),
        None => format!("Could not evaluate: {}", expression),
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
        Err(e) => return format!("Geocoding failed: {}", e),
    };

    let geo_data: serde_json::Value = match geo_resp.json().await {
        Ok(d) => d,
        Err(e) => return format!("Failed to parse geocoding: {}", e),
    };

    let results = match geo_data.get("results").and_then(|v| v.as_array()) {
        Some(r) if !r.is_empty() => r,
        _ => return format!("Location not found: {}", location),
    };

    let lat = results[0].get("latitude").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let lon = results[0].get("longitude").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let name = results[0].get("name").and_then(|v| v.as_str()).unwrap_or(location);

    // Get weather
    let weather_url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={}&longitude={}&current=temperature_2m,relative_humidity_2m,wind_speed_10m,weather_code&timezone=auto",
        lat, lon
    );

    let weather_resp = match client.get(&weather_url).send().await {
        Ok(r) => r,
        Err(e) => return format!("Weather fetch failed: {}", e),
    };

    let weather: serde_json::Value = match weather_resp.json().await {
        Ok(d) => d,
        Err(e) => return format!("Failed to parse weather: {}", e),
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
        "Weather in {}:\n- Temperature: {:.1}°C\n- Condition: {}\n- Humidity: {:.0}%\n- Wind: {:.1} km/h",
        name, temp, condition, humidity, wind
    )
}

/// Strip HTML tags from text, removing script/style content entirely.
fn strip_html_tags(html: &str) -> String {
    // First, remove <script>...</script> and <style>...</style> blocks entirely
    let mut clean = html.to_string();
    for tag in &["script", "style", "noscript", "svg", "head"] {
        loop {
            let open = format!("<{}", tag);
            let close = format!("</{}>", tag);
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
            description: "Manage GitHub issues and PRs".to_string(),
            enabled: false,
            requires_auth: true,
            auth_url: Some("https://github.com/login/oauth/authorize".to_string()),
        },
    ]
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
        let registry = ToolRegistry::with_builtins();
        assert_eq!(registry.len(), 4);
        let defs = registry.get_definitions();
        let names: Vec<&str> = defs.iter()
            .filter_map(|t| t.pointer("/function/name").and_then(|v| v.as_str()))
            .collect();
        assert!(names.contains(&"web_search"));
        assert!(names.contains(&"web_fetch"));
        assert!(names.contains(&"calculator"));
        assert!(names.contains(&"weather"));
    }

    #[test]
    fn test_tool_registry_definitions_match_legacy() {
        let legacy = get_tool_definitions();
        let registry = ToolRegistry::with_builtins();
        let new = registry.get_definitions();
        // Same number of tools
        assert_eq!(legacy.len(), new.len());
        // Same names in same order
        for (l, n) in legacy.iter().zip(new.iter()) {
            assert_eq!(
                l.pointer("/function/name"),
                n.pointer("/function/name"),
            );
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
