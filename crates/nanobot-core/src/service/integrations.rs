//! External API integrations for chatweb.ai
//!
//! Provides tool definitions and execution for external services.
//! Each integration registers tools that the AI can call during conversations.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

/// Web search using DuckDuckGo instant answer API.
async fn execute_web_search(query: &str) -> String {
    // Try Brave Search API first (if key is available), then fall back to DuckDuckGo HTML
    if let Ok(brave_key) = std::env::var("BRAVE_API_KEY") {
        if let Some(result) = brave_search(query, &brave_key).await {
            return result;
        }
    }

    // Fall back to direct site search (DuckDuckGo blocks cloud IPs with CAPTCHAs)
    direct_site_search(query).await
}

/// Search using Brave Search API (returns high-quality results with URLs).
async fn brave_search(query: &str, api_key: &str) -> Option<String> {
    let client = reqwest::Client::new();
    let url = format!(
        "https://api.search.brave.com/res/v1/web/search?q={}&count=8",
        urlencoding::encode(query)
    );

    let resp = client.get(&url)
        .header("Accept", "application/json")
        .header("X-Subscription-Token", api_key)
        .send()
        .await.ok()?;

    let data: serde_json::Value = resp.json().await.ok()?;
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

/// Fallback search: fetch kakaku.com (Japan's biggest price comparison site) for product info.
/// This works even when search engines block cloud IPs with CAPTCHAs.
async fn direct_site_search(query: &str) -> String {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    // Clean the query: strip "site:" prefixes and common search operators
    let clean_query: String = query.split_whitespace()
        .filter(|w| !w.starts_with("site:") && !w.starts_with("-"))
        .collect::<Vec<_>>()
        .join(" ");
    let encoded = urlencoding::encode(&clean_query);

    // Use kakaku.com (price comparison) and Amazon as primary sources
    let search_urls = vec![
        (format!("https://kakaku.com/search_results/{}/?query={}", encoded, encoded), "kakaku.com"),
        (format!("https://www.amazon.co.jp/s?k={}", encoded), "Amazon.co.jp"),
    ];

    // Fetch in parallel
    let mut handles = Vec::new();
    for (url, source) in search_urls {
        let client = client.clone();
        let source = source.to_string();
        handles.push(tokio::spawn(async move {
            match client.get(&url)
                .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
                .header("Accept-Language", "ja,en;q=0.9")
                .send()
                .await
            {
                Ok(resp) => {
                    let final_url = resp.url().to_string();
                    if resp.status().is_success() {
                        if let Ok(body) = resp.text().await {
                            let text = strip_html_tags(&body);
                            let cleaned: String = text.lines()
                                .map(|l| l.trim())
                                .filter(|l| !l.is_empty() && l.len() > 3)
                                .take(40)
                                .collect::<Vec<_>>()
                                .join("\n");
                            let snippet = if cleaned.len() > 1500 { &cleaned[..1500] } else { &cleaned };
                            return Some(format!("[{}] URL: {}\n{}", source, final_url, snippet));
                        }
                    }
                    None
                }
                Err(_) => None,
            }
        }));
    }

    let mut results = Vec::new();
    for h in handles {
        if let Ok(Some(result)) = h.await {
            results.push(result);
        }
    }

    if results.is_empty() {
        format!("No results found for: {}. Try using web_fetch with specific product URLs.", query)
    } else {
        format!("Search results for \"{}\":\n\n{}", query, results.join("\n\n---\n\n"))
    }
}

/// Fetch a web page and extract text content.
async fn execute_web_fetch(url: &str) -> String {
    if url.is_empty() {
        return "No URL provided".to_string();
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    match client.get(url)
        .header("User-Agent", "Mozilla/5.0 (compatible; chatweb.ai bot)")
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .header("Accept-Language", "ja,en;q=0.9")
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            if !status.is_success() {
                return format!("HTTP {} for {}", status, url);
            }
            match resp.text().await {
                Ok(body) => {
                    // Simple HTML to text: strip tags
                    let text = strip_html_tags(&body);
                    // Clean up excessive whitespace
                    let cleaned: String = text.lines()
                        .map(|l| l.trim())
                        .filter(|l| !l.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n");
                    // Limit length
                    if cleaned.len() > 8000 {
                        format!("Content from {}:\n\n{}...\n\n[Truncated, {} chars total]", url, &cleaned[..8000], cleaned.len())
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

/// Strip HTML tags from text.
fn strip_html_tags(html: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    let in_script = false;
    let mut last_was_space = false;

    for c in html.chars() {
        if c == '<' {
            in_tag = true;
            continue;
        }
        if c == '>' {
            in_tag = false;
            continue;
        }
        if in_tag {
            continue;
        }
        if in_script {
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

    let _ = in_script; // suppress warning
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
        // Web search and calculator should be enabled by default
        let enabled: Vec<&str> = integrations.iter()
            .filter(|i| i.enabled)
            .map(|i| i.name.as_str())
            .collect();
        assert!(enabled.contains(&"Web Search"));
        assert!(enabled.contains(&"Calculator"));
    }
}
