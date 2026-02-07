use async_trait::async_trait;
use regex::Regex;
use serde_json::json;
use std::collections::HashMap;

use crate::util::http;
use super::Tool;

const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_7_2) AppleWebKit/537.36";

/// Search the web using Brave Search API.
pub struct WebSearchTool {
    api_key: String,
    max_results: u32,
}

impl WebSearchTool {
    pub fn new(api_key: Option<String>, max_results: u32) -> Self {
        Self {
            api_key: api_key
                .or_else(|| std::env::var("BRAVE_API_KEY").ok())
                .unwrap_or_default(),
            max_results,
        }
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web. Returns titles, URLs, and snippets."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Search query"},
                "count": {"type": "integer", "description": "Results (1-10)", "minimum": 1, "maximum": 10}
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        if self.api_key.is_empty() {
            return "Error: BRAVE_API_KEY not configured".to_string();
        }

        let query = match params.get("query").and_then(|v| v.as_str()) {
            Some(q) => q,
            None => return "Error: 'query' parameter is required".to_string(),
        };

        let count = params
            .get("count")
            .and_then(|v| v.as_u64())
            .unwrap_or(self.max_results as u64)
            .min(10)
            .max(1);

        match http::client()
            .get("https://api.search.brave.com/res/v1/web/search")
            .query(&[("q", query), ("count", &count.to_string())])
            .header("Accept", "application/json")
            .header("X-Subscription-Token", &self.api_key)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
        {
            Ok(response) => {
                if !response.status().is_success() {
                    return format!("Error: Search API returned {}", response.status());
                }
                match response.json::<serde_json::Value>().await {
                    Ok(data) => {
                        let results = data
                            .get("web")
                            .and_then(|w| w.get("results"))
                            .and_then(|r| r.as_array());
                        match results {
                            Some(results) if !results.is_empty() => {
                                let mut lines = vec![format!("Results for: {}\n", query)];
                                for (i, item) in results.iter().take(count as usize).enumerate() {
                                    let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("");
                                    let url = item.get("url").and_then(|v| v.as_str()).unwrap_or("");
                                    lines.push(format!("{}. {}\n   {}", i + 1, title, url));
                                    if let Some(desc) = item.get("description").and_then(|v| v.as_str()) {
                                        lines.push(format!("   {}", desc));
                                    }
                                }
                                lines.join("\n")
                            }
                            _ => format!("No results for: {}", query),
                        }
                    }
                    Err(e) => format!("Error parsing response: {}", e),
                }
            }
            Err(e) => format!("Error: {}", e),
        }
    }
}

/// Fetch and extract content from a URL.
pub struct WebFetchTool {
    max_chars: usize,
}

impl WebFetchTool {
    pub fn new(max_chars: usize) -> Self {
        Self { max_chars }
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "Fetch URL and extract readable content (HTML -> text)."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "URL to fetch"},
                "maxChars": {"type": "integer", "minimum": 100}
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let url = match params.get("url").and_then(|v| v.as_str()) {
            Some(u) => u,
            None => return json!({"error": "'url' parameter is required"}).to_string(),
        };

        let max_chars = params
            .get("maxChars")
            .and_then(|v| v.as_u64())
            .unwrap_or(self.max_chars as u64) as usize;

        // Validate URL
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return json!({"error": "Only http/https URLs allowed", "url": url}).to_string();
        }

        match http::client()
            .get(url)
            .header("User-Agent", USER_AGENT)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
        {
            Ok(response) => {
                let status = response.status().as_u16();
                let final_url = response.url().to_string();
                let content_type = response
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();

                match response.text().await {
                    Ok(body) => {
                        let (text, extractor) = if content_type.contains("application/json") {
                            // Try to pretty-print JSON
                            let formatted = serde_json::from_str::<serde_json::Value>(&body)
                                .ok()
                                .and_then(|v| serde_json::to_string_pretty(&v).ok())
                                .unwrap_or(body);
                            (formatted, "json")
                        } else if content_type.contains("text/html")
                            || body.trim_start().to_lowercase().starts_with("<!doctype")
                            || body.trim_start().to_lowercase().starts_with("<html")
                        {
                            (strip_html_tags(&body), "html_strip")
                        } else {
                            (body, "raw")
                        };

                        let truncated = text.len() > max_chars;
                        let text = if truncated {
                            text[..max_chars].to_string()
                        } else {
                            text
                        };

                        json!({
                            "url": url,
                            "finalUrl": final_url,
                            "status": status,
                            "extractor": extractor,
                            "truncated": truncated,
                            "length": text.len(),
                            "text": text,
                        })
                        .to_string()
                    }
                    Err(e) => json!({"error": e.to_string(), "url": url}).to_string(),
                }
            }
            Err(e) => json!({"error": e.to_string(), "url": url}).to_string(),
        }
    }
}

/// Strip HTML tags and normalize whitespace.
fn strip_html_tags(html: &str) -> String {
    // Remove script and style blocks
    let re_script = Regex::new(r"(?is)<script[\s\S]*?</script>").unwrap();
    let re_style = Regex::new(r"(?is)<style[\s\S]*?</style>").unwrap();
    let re_tags = Regex::new(r"<[^>]+>").unwrap();
    let re_spaces = Regex::new(r"[ \t]+").unwrap();
    let re_newlines = Regex::new(r"\n{3,}").unwrap();

    let text = re_script.replace_all(html, "");
    let text = re_style.replace_all(&text, "");
    let text = re_tags.replace_all(&text, "");
    let text = re_spaces.replace_all(&text, " ");
    let text = re_newlines.replace_all(&text, "\n\n");

    // Decode common HTML entities
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .trim()
        .to_string()
}
