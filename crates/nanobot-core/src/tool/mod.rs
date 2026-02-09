pub mod filesystem;
pub mod shell;
pub mod web;
pub mod message;
pub mod spawn;
pub mod cron_tool;

use async_trait::async_trait;
use dashmap::DashMap;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::debug;

/// Trait for agent tools.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name used in function calls.
    fn name(&self) -> &str;

    /// Description of what the tool does.
    fn description(&self) -> &str;

    /// JSON Schema for tool parameters.
    fn parameters(&self) -> serde_json::Value;

    /// Execute the tool with given parameters.
    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String;
}

/// Extension trait for Tool to convert to OpenAI function schema.
pub trait ToolSchema: Tool {
    fn to_schema(&self) -> serde_json::Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": self.description(),
                "parameters": self.parameters(),
            }
        })
    }
}

impl<T: Tool + ?Sized> ToolSchema for T {}

/// Lock-free tool registry using DashMap.
pub struct ToolRegistry {
    tools: DashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: DashMap::new(),
        }
    }

    /// Register a tool.
    pub fn register(&self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Unregister a tool by name.
    pub fn unregister(&self, name: &str) {
        self.tools.remove(name);
    }

    /// Get a tool by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).map(|r| r.value().clone())
    }

    /// Check if a tool is registered.
    pub fn has(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Get all tool definitions in OpenAI format.
    pub fn get_definitions(&self) -> Vec<serde_json::Value> {
        self.tools
            .iter()
            .map(|entry| entry.value().to_schema())
            .collect()
    }

    /// Execute a tool by name with given parameters.
    pub async fn execute(
        &self,
        name: &str,
        params: HashMap<String, serde_json::Value>,
    ) -> String {
        let tool = match self.tools.get(name) {
            Some(t) => t.value().clone(),
            None => return format!("Error: Tool '{name}' not found"),
        };

        debug!("Executing tool: {}", name);
        tool.execute(params).await
    }

    /// Execute multiple tools concurrently (join_all).
    pub async fn execute_parallel(
        &self,
        calls: Vec<(String, HashMap<String, serde_json::Value>)>,
    ) -> Vec<(String, String)> {
        let futures: Vec<_> = calls
            .into_iter()
            .map(|(name, params)| {
                let registry = &self;
                let name_clone = name.clone();
                async move {
                    let result = registry.execute(&name, params).await;
                    (name_clone, result)
                }
            })
            .collect();

        futures::future::join_all(futures).await
    }

    /// Get list of registered tool names.
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.iter().map(|e| e.key().clone()).collect()
    }

    /// Number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
