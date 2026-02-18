use async_trait::async_trait;
use serde_json::json;
use std::collections::HashMap;
use tracing::debug;

use crate::error::ProviderError;
use crate::types::{CompletionResponse, FinishReason, Message, Role, TokenUsage, ToolCall};
use crate::util::http;

use super::LlmProvider;

/// Google Gemini API provider.
pub struct GeminiProvider {
    api_key: String,
    api_base: String,
    default_model: String,
}

impl GeminiProvider {
    pub fn new(api_key: String, api_base: Option<String>, default_model: String) -> Self {
        let base = api_base
            .unwrap_or_else(|| "https://generativelanguage.googleapis.com/v1beta".to_string());
        Self {
            api_key,
            api_base: base.trim_end_matches('/').to_string(),
            default_model,
        }
    }

    /// Normalize model name: strip "gemini/" prefix.
    fn normalize_model(&self, model: &str) -> String {
        model
            .strip_prefix("gemini/")
            .unwrap_or(model)
            .to_string()
    }

    /// Convert messages to Gemini format.
    fn convert_messages(
        &self,
        messages: &[Message],
    ) -> (Option<serde_json::Value>, Vec<serde_json::Value>) {
        let mut system_instruction = None;
        let mut contents = Vec::new();

        for msg in messages {
            match msg.role {
                Role::System => {
                    system_instruction = Some(json!({
                        "parts": [{"text": msg.content.as_deref().unwrap_or("")}]
                    }));
                }
                Role::User => {
                    contents.push(json!({
                        "role": "user",
                        "parts": [{"text": msg.content.as_deref().unwrap_or("")}]
                    }));
                }
                Role::Assistant => {
                    let mut parts = Vec::new();
                    if let Some(ref text) = msg.content {
                        if !text.is_empty() {
                            parts.push(json!({"text": text}));
                        }
                    }
                    if let Some(ref tool_calls) = msg.tool_calls {
                        for tc in tool_calls {
                            if let Some(tc_obj) = tc.as_object() {
                                let function = tc_obj.get("function").unwrap_or(tc);
                                let name = function
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                let args_str = function
                                    .get("arguments")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("{}");
                                let args: serde_json::Value =
                                    serde_json::from_str(args_str).unwrap_or(json!({}));
                                parts.push(json!({
                                    "functionCall": {"name": name, "args": args}
                                }));
                            }
                        }
                    }
                    if parts.is_empty() {
                        parts.push(json!({"text": ""}));
                    }
                    contents.push(json!({"role": "model", "parts": parts}));
                }
                Role::Tool => {
                    let name = msg.name.as_deref().unwrap_or("tool");
                    let content = msg.content.as_deref().unwrap_or("");
                    // Try to parse content as JSON, fallback to wrapping in result
                    let response: serde_json::Value = serde_json::from_str(content)
                        .unwrap_or_else(|_| json!({"result": content}));
                    contents.push(json!({
                        "role": "user",
                        "parts": [{
                            "functionResponse": {
                                "name": name,
                                "response": response,
                            }
                        }]
                    }));
                }
            }
        }

        (system_instruction, contents)
    }

    /// Convert OpenAI-format tool definitions to Gemini format.
    fn convert_tools(&self, tools: &[serde_json::Value]) -> Vec<serde_json::Value> {
        let declarations: Vec<serde_json::Value> = tools
            .iter()
            .filter_map(|t| {
                let function = t.get("function")?;
                Some(json!({
                    "name": function.get("name")?,
                    "description": function.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                    "parameters": function.get("parameters").cloned().unwrap_or(json!({"type": "object", "properties": {}})),
                }))
            })
            .collect();

        vec![json!({"functionDeclarations": declarations})]
    }
}

#[async_trait]
impl LlmProvider for GeminiProvider {
    async fn chat(
        &self,
        messages: &[Message],
        tools: Option<&[serde_json::Value]>,
        model: &str,
        max_tokens: u32,
        temperature: f64,
    ) -> Result<CompletionResponse, ProviderError> {
        let model_name = self.normalize_model(model);
        let url = format!(
            "{}/models/{}:generateContent?key={}",
            self.api_base, model_name, self.api_key
        );

        let (system_instruction, contents) = self.convert_messages(messages);

        let mut body = json!({
            "contents": contents,
            "generationConfig": {
                "maxOutputTokens": max_tokens,
                "temperature": temperature,
            },
        });

        if let Some(system) = &system_instruction {
            body["systemInstruction"] = system.clone();
        }

        if let Some(tools) = tools {
            if !tools.is_empty() {
                body["tools"] = json!(self.convert_tools(tools));
            }
        }

        debug!("Gemini request with model {}", model_name);

        let response = http::client()
            .post(&url)
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
        self.parse_response(&data)
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }
}

impl GeminiProvider {
    fn parse_response(&self, data: &serde_json::Value) -> Result<CompletionResponse, ProviderError> {
        let candidate = data
            .get("candidates")
            .and_then(|v| v.get(0))
            .ok_or_else(|| ProviderError::Parse("No candidates in response".to_string()))?;

        let parts = candidate
            .get("content")
            .and_then(|v| v.get("parts"))
            .and_then(|v| v.as_array())
            .ok_or_else(|| ProviderError::Parse("No parts in response".to_string()))?;

        let mut text_content = String::new();
        let mut tool_calls = Vec::new();

        for part in parts {
            if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                text_content.push_str(text);
            }
            if let Some(fc) = part.get("functionCall") {
                let name = fc
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let args = fc.get("args").cloned().unwrap_or(json!({}));
                let arguments: HashMap<String, serde_json::Value> =
                    serde_json::from_value(args).unwrap_or_default();

                tool_calls.push(ToolCall {
                    id: format!("call_{}", uuid::Uuid::new_v4().to_string().split('-').next().unwrap_or("0")),
                    name,
                    arguments,
                });
            }
        }

        let finish_reason = match candidate.get("finishReason").and_then(|v| v.as_str()) {
            Some("STOP") => FinishReason::Stop,
            Some("MAX_TOKENS") => FinishReason::Length,
            _ if !tool_calls.is_empty() => FinishReason::ToolCalls,
            _ => FinishReason::Stop,
        };

        let usage = if let Some(u) = data.get("usageMetadata") {
            TokenUsage {
                prompt_tokens: u
                    .get("promptTokenCount")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32,
                completion_tokens: u
                    .get("candidatesTokenCount")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32,
                total_tokens: u
                    .get("totalTokenCount")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32,
            }
        } else {
            TokenUsage::default()
        };

        Ok(CompletionResponse {
            content: if text_content.is_empty() {
                None
            } else {
                Some(text_content)
            },
            tool_calls,
            finish_reason,
            usage,
        })
    }
}
