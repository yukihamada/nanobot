//! MCP (Model Context Protocol) client for connecting to external tool servers.
//!
//! Uses HTTP Streamable transport (Lambda-compatible — no STDIO).
//! Implements JSON-RPC protocol per MCP 2025-11-25 spec.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::service::integrations::Tool;

/// A tool definition received from an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

/// JSON-RPC request.
#[derive(Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

/// JSON-RPC response.
#[derive(Deserialize)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    jsonrpc: String,
    #[allow(dead_code)]
    id: Option<u64>,
    result: Option<serde_json::Value>,
    error: Option<JsonRpcError>,
}

#[derive(Deserialize)]
struct JsonRpcError {
    #[allow(dead_code)]
    code: i64,
    message: String,
}

/// MCP client for a single MCP server.
pub struct McpClient {
    endpoint: String,
    name: String,
    client: reqwest::Client,
}

impl McpClient {
    /// Create a new MCP client for the given server endpoint.
    pub fn new(name: &str, endpoint: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            endpoint: endpoint.to_string(),
            name: name.to_string(),
            client,
        }
    }

    /// Send a JSON-RPC request to the MCP server.
    async fn rpc_call(&self, method: &str, params: Option<serde_json::Value>) -> Result<serde_json::Value, String> {
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: method.to_string(),
            params,
        };

        let resp = self.client.post(&self.endpoint)
            .header("Content-Type", "application/json")
            .json(&req)
            .send()
            .await
            .map_err(|e| format!("MCP request failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("MCP server returned HTTP {}", resp.status()));
        }

        let rpc_resp: JsonRpcResponse = resp.json()
            .await
            .map_err(|e| format!("MCP response parse error: {e}"))?;

        if let Some(err) = rpc_resp.error {
            return Err(format!("MCP error: {}", err.message));
        }

        rpc_resp.result.ok_or_else(|| "MCP response has no result".to_string())
    }

    /// Initialize the MCP session.
    pub async fn initialize(&self) -> Result<(), String> {
        let params = serde_json::json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {
                "name": "chatweb.ai",
                "version": env!("CARGO_PKG_VERSION"),
            }
        });
        self.rpc_call("initialize", Some(params)).await?;
        // Send initialized notification (no response expected, but we fire it)
        let _ = self.rpc_call("notifications/initialized", None).await;
        Ok(())
    }

    /// List available tools from the MCP server.
    pub async fn list_tools(&self) -> Vec<McpToolDef> {
        match self.rpc_call("tools/list", None).await {
            Ok(result) => {
                #[derive(Deserialize)]
                struct ToolsListResult {
                    tools: Vec<McpToolDef>,
                }
                match serde_json::from_value::<ToolsListResult>(result) {
                    Ok(r) => r.tools,
                    Err(e) => {
                        tracing::warn!("MCP {} tools/list parse error: {}", self.name, e);
                        vec![]
                    }
                }
            }
            Err(e) => {
                tracing::warn!("MCP {} tools/list failed: {}", self.name, e);
                vec![]
            }
        }
    }

    /// Call a tool on the MCP server.
    pub async fn call_tool(&self, name: &str, args: serde_json::Value) -> String {
        let params = serde_json::json!({
            "name": name,
            "arguments": args,
        });

        match self.rpc_call("tools/call", Some(params)).await {
            Ok(result) => {
                // MCP returns { content: [{ type: "text", text: "..." }] }
                if let Some(content) = result.get("content").and_then(|c| c.as_array()) {
                    content.iter()
                        .filter_map(|c| c.get("text").and_then(|t| t.as_str()))
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    result.to_string()
                }
            }
            Err(e) => format!("MCP tool error: {e}"),
        }
    }

    /// Server name for display.
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Wrapper that implements the Tool trait for an MCP tool.
pub struct McpTool {
    client: Arc<McpClient>,
    tool_def: McpToolDef,
}

impl McpTool {
    pub fn new(client: Arc<McpClient>, tool_def: McpToolDef) -> Self {
        Self { client, tool_def }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.tool_def.name
    }

    fn description(&self) -> &str {
        &self.tool_def.description
    }

    fn parameters(&self) -> serde_json::Value {
        self.tool_def.input_schema.clone()
    }

    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        self.client.call_tool(&self.tool_def.name, serde_json::json!(params)).await
    }
}

/// Parse MCP_SERVERS env var and create McpTool instances.
/// Format: "name1:url1,name2:url2"
pub async fn load_mcp_tools_from_env() -> Vec<Box<dyn Tool>> {
    let servers_env = match std::env::var("MCP_SERVERS") {
        Ok(v) if !v.is_empty() => v,
        _ => return vec![],
    };

    let mut tools: Vec<Box<dyn Tool>> = vec![];

    for entry in servers_env.split(',') {
        let entry = entry.trim();
        if entry.is_empty() { continue; }

        let parts: Vec<&str> = entry.splitn(2, ':').collect();
        if parts.len() < 2 {
            tracing::warn!("MCP_SERVERS: invalid entry '{}', expected 'name:url'", entry);
            continue;
        }

        let name = parts[0].trim();
        // Rejoin in case the URL has colons (http:// etc.)
        let url = entry[name.len() + 1..].trim();

        tracing::info!("MCP: connecting to server '{}' at {}", name, url);

        let client = Arc::new(McpClient::new(name, url));

        // Initialize the session
        if let Err(e) = client.initialize().await {
            tracing::warn!("MCP {} initialization failed: {}. Skipping.", name, e);
            continue;
        }

        // List tools
        let tool_defs = client.list_tools().await;
        tracing::info!("MCP {}: loaded {} tools", name, tool_defs.len());

        for def in tool_defs {
            tracing::info!("MCP {}: registered tool '{}'", name, def.name);
            tools.push(Box::new(McpTool::new(client.clone(), def)));
        }
    }

    tracing::info!("MCP: loaded {} total tools from external servers", tools.len());
    tools
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_tool_def_parse() {
        let json = r#"{"name": "test_tool", "description": "A test", "inputSchema": {"type": "object"}}"#;
        let def: McpToolDef = serde_json::from_str(json).unwrap();
        assert_eq!(def.name, "test_tool");
        assert_eq!(def.description, "A test");
    }

    #[test]
    fn test_mcp_client_new() {
        let client = McpClient::new("test", "https://example.com/mcp");
        assert_eq!(client.name(), "test");
    }
}
