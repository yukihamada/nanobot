use async_trait::async_trait;
use serde_json::json;
use std::collections::HashMap;
use tracing::debug;

use crate::error::ProviderError;
use crate::types::{CompletionResponse, FinishReason, Message, TokenUsage, ToolCall};
use crate::util::http;

use super::LlmProvider;

/// OpenAI-compatible provider.
/// Works with OpenRouter, DeepSeek, Groq, Moonshot, vLLM, and any OpenAI-compatible API.
pub struct OpenAiCompatProvider {
    api_key: String,
    api_base: String,
    default_model: String,
}

impl OpenAiCompatProvider {
    pub fn new(api_key: String, api_base: Option<String>, default_model: String) -> Self {
        let base = api_base.unwrap_or_else(|| {
            let model = default_model.to_lowercase();
            if model.contains("openrouter") {
                "https://openrouter.ai/api/v1".to_string()
            } else if model.contains("deepseek") {
                "https://api.deepseek.com/v1".to_string()
            } else if model.contains("groq") {
                "https://api.groq.com/openai/v1".to_string()
            } else if model.contains("moonshot") || model.contains("kimi") {
                "https://api.moonshot.cn/v1".to_string()
            } else {
                "https://api.openai.com/v1".to_string()
            }
        });

        Self {
            api_key,
            api_base: base.trim_end_matches('/').to_string(),
            default_model,
        }
    }

    /// Normalize model name for the API (strip provider prefixes).
    fn normalize_model(&self, model: &str) -> String {
        let m = model.to_string();
        // Strip common prefixes like "openrouter/", "deepseek/", etc.
        // OpenRouter expects the model name without prefix
        if let Some(rest) = m.strip_prefix("openrouter/") {
            rest.to_string()
        } else {
            m
        }
    }
}

#[async_trait]
impl LlmProvider for OpenAiCompatProvider {
    async fn chat(
        &self,
        messages: &[Message],
        tools: Option<&[serde_json::Value]>,
        model: &str,
        max_tokens: u32,
        temperature: f64,
    ) -> Result<CompletionResponse, ProviderError> {
        let url = format!("{}/chat/completions", self.api_base);
        let model_name = self.normalize_model(model);

        // Build messages array
        let msgs: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| {
                let mut msg = json!({
                    "role": m.role,
                    "content": m.content.as_deref().unwrap_or(""),
                });
                if let Some(ref tc) = m.tool_calls {
                    msg["tool_calls"] = json!(tc);
                }
                if let Some(ref id) = m.tool_call_id {
                    msg["tool_call_id"] = json!(id);
                }
                if let Some(ref name) = m.name {
                    msg["name"] = json!(name);
                }
                msg
            })
            .collect();

        let mut body = json!({
            "model": model_name,
            "messages": msgs,
            "max_tokens": max_tokens,
            "temperature": temperature,
        });

        if let Some(tools) = tools {
            if !tools.is_empty() {
                body["tools"] = json!(tools);
                body["tool_choice"] = json!("auto");
            }
        }

        debug!("OpenAI-compat request to {} with model {}", url, model_name);

        let response = http::client()
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(ProviderError::Api {
                status: status.as_u16(),
                message: text,
            });
        }

        let data: serde_json::Value = response.json().await?;
        parse_openai_response(&data)
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }
}

/// Parse an OpenAI-format response into our CompletionResponse.
pub fn parse_openai_response(data: &serde_json::Value) -> Result<CompletionResponse, ProviderError> {
    let choice = data
        .get("choices")
        .and_then(|c| c.get(0))
        .ok_or_else(|| ProviderError::Parse("No choices in response".to_string()))?;

    let message = choice
        .get("message")
        .ok_or_else(|| ProviderError::Parse("No message in choice".to_string()))?;

    let content = message.get("content").and_then(|v| v.as_str()).map(|s| s.to_string());

    let finish_reason = match choice.get("finish_reason").and_then(|v| v.as_str()) {
        Some("stop") => FinishReason::Stop,
        Some("tool_calls") => FinishReason::ToolCalls,
        Some("length") => FinishReason::Length,
        _ => FinishReason::Stop,
    };

    let mut tool_calls = Vec::new();
    if let Some(tcs) = message.get("tool_calls").and_then(|v| v.as_array()) {
        for tc in tcs {
            let id = tc
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let function = tc.get("function").unwrap_or(tc);
            let name = function
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let args_str = function
                .get("arguments")
                .and_then(|v| v.as_str())
                .unwrap_or("{}");
            let arguments: HashMap<String, serde_json::Value> =
                serde_json::from_str(args_str).unwrap_or_else(|_| {
                    let mut m = HashMap::new();
                    m.insert("raw".to_string(), serde_json::Value::String(args_str.to_string()));
                    m
                });

            tool_calls.push(ToolCall {
                id,
                name,
                arguments,
            });
        }
    }

    let usage = if let Some(u) = data.get("usage") {
        TokenUsage {
            prompt_tokens: u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            completion_tokens: u
                .get("completion_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            total_tokens: u.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        }
    } else {
        TokenUsage::default()
    };

    Ok(CompletionResponse {
        content,
        tool_calls,
        finish_reason,
        usage,
    })
}
