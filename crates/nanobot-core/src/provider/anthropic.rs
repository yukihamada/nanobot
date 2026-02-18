use async_trait::async_trait;
use serde_json::json;
use std::collections::HashMap;
use tracing::debug;

use crate::error::ProviderError;
use crate::types::{CompletionResponse, FinishReason, Message, Role, TokenUsage, ToolCall};
use crate::util::http;

use super::{LlmProvider, ChatExtra};

/// Native Anthropic Messages API provider.
pub struct AnthropicProvider {
    api_key: String,
    api_base: String,
    default_model: String,
}

impl AnthropicProvider {
    pub fn new(api_key: String, api_base: Option<String>, default_model: String) -> Self {
        let base = api_base.unwrap_or_else(|| "https://api.anthropic.com".to_string());
        Self {
            api_key,
            api_base: base.trim_end_matches('/').to_string(),
            default_model,
        }
    }

    /// Normalize model name: strip "anthropic/" prefix.
    fn normalize_model(&self, model: &str) -> String {
        model
            .strip_prefix("anthropic/")
            .unwrap_or(model)
            .to_string()
    }

    /// Convert our generic messages to Anthropic format.
    /// Anthropic expects system as a separate top-level field,
    /// and tool results use a different format.
    fn convert_messages(
        &self,
        messages: &[Message],
    ) -> (Option<String>, Vec<serde_json::Value>) {
        let mut system_prompt = None;
        let mut converted = Vec::new();

        for msg in messages {
            match msg.role {
                Role::System => {
                    system_prompt = msg.content.clone();
                }
                Role::User => {
                    converted.push(json!({
                        "role": "user",
                        "content": msg.content.as_deref().unwrap_or(""),
                    }));
                }
                Role::Assistant => {
                    let mut assistant_msg = json!({
                        "role": "assistant",
                    });

                    // Build content blocks for Anthropic format
                    let mut content_blocks = Vec::new();
                    if let Some(ref text) = msg.content {
                        if !text.is_empty() {
                            content_blocks.push(json!({"type": "text", "text": text}));
                        }
                    }

                    // Convert tool_calls to tool_use blocks
                    if let Some(ref tool_calls) = msg.tool_calls {
                        for tc in tool_calls {
                            if let Some(tc_obj) = tc.as_object() {
                                let id = tc_obj
                                    .get("id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                let function = tc_obj.get("function").unwrap_or(tc);
                                let name = function
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                let args_str = function
                                    .get("arguments")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("{}");
                                let input: serde_json::Value =
                                    serde_json::from_str(args_str).unwrap_or(json!({}));

                                content_blocks.push(json!({
                                    "type": "tool_use",
                                    "id": id,
                                    "name": name,
                                    "input": input,
                                }));
                            }
                        }
                    }

                    if content_blocks.is_empty() {
                        assistant_msg["content"] = json!("");
                    } else {
                        assistant_msg["content"] = json!(content_blocks);
                    }

                    converted.push(assistant_msg);
                }
                Role::Tool => {
                    // Anthropic tool results go in a user message with tool_result blocks
                    let tool_call_id = msg.tool_call_id.as_deref().unwrap_or("");
                    converted.push(json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": tool_call_id,
                            "content": msg.content.as_deref().unwrap_or(""),
                        }],
                    }));
                }
            }
        }

        (system_prompt, converted)
    }

    /// Convert OpenAI-format tool definitions to Anthropic format.
    fn convert_tools(&self, tools: &[serde_json::Value]) -> Vec<serde_json::Value> {
        tools
            .iter()
            .filter_map(|t| {
                let function = t.get("function")?;
                Some(json!({
                    "name": function.get("name")?,
                    "description": function.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                    "input_schema": function.get("parameters").cloned().unwrap_or(json!({"type": "object", "properties": {}})),
                }))
            })
            .collect()
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn chat(
        &self,
        messages: &[Message],
        tools: Option<&[serde_json::Value]>,
        model: &str,
        max_tokens: u32,
        temperature: f64,
    ) -> Result<CompletionResponse, ProviderError> {
        let url = format!("{}/v1/messages", self.api_base);
        let model_name = self.normalize_model(model);

        let (system_prompt, msgs) = self.convert_messages(messages);

        let mut body = json!({
            "model": model_name,
            "messages": msgs,
            "max_tokens": max_tokens,
            "temperature": temperature,
        });

        if let Some(system) = &system_prompt {
            body["system"] = json!(system);
        }

        if let Some(tools) = tools {
            if !tools.is_empty() {
                body["tools"] = json!(self.convert_tools(tools));
                let has_tool_results = messages.iter().any(|m| m.role == crate::types::Role::Tool);
                body["tool_choice"] = if has_tool_results {
                    json!({"type": "auto"})
                } else {
                    json!({"type": "any"})
                };
            }
        }

        debug!("Anthropic request to {} with model {}", url, model_name);

        let response = http::client()
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
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

    async fn chat_with_extra(
        &self,
        messages: &[Message],
        tools: Option<&[serde_json::Value]>,
        model: &str,
        max_tokens: u32,
        temperature: f64,
        extra: &ChatExtra,
    ) -> Result<CompletionResponse, ProviderError> {
        let url = format!("{}/v1/messages", self.api_base);
        let model_name = self.normalize_model(model);
        let (system_prompt, msgs) = self.convert_messages(messages);

        let mut body = json!({
            "model": model_name,
            "messages": msgs,
            "max_tokens": max_tokens,
            "temperature": temperature,
        });

        // Anthropic supports top_p but not frequency/presence penalty
        if let Some(top_p) = extra.top_p {
            body["top_p"] = json!(top_p);
        }

        if let Some(system) = &system_prompt {
            body["system"] = json!(system);
        }

        if let Some(tools) = tools {
            if !tools.is_empty() {
                body["tools"] = json!(self.convert_tools(tools));
                let has_tool_results = messages.iter().any(|m| m.role == crate::types::Role::Tool);
                body["tool_choice"] = if has_tool_results {
                    json!({"type": "auto"})
                } else {
                    json!({"type": "any"})
                };
            }
        }

        debug!("Anthropic request (with extra) to {} with model {}", url, model_name);

        let response = http::client()
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
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

    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: Option<&[serde_json::Value]>,
        model: &str,
        max_tokens: u32,
        temperature: f64,
        extra: &ChatExtra,
        chunk_tx: tokio::sync::mpsc::UnboundedSender<String>,
    ) -> Result<CompletionResponse, ProviderError> {
        use futures::StreamExt;

        let url = format!("{}/v1/messages", self.api_base);
        let model_name = self.normalize_model(model);
        let (system_prompt, msgs) = self.convert_messages(messages);

        let mut body = json!({
            "model": model_name, "messages": msgs,
            "max_tokens": max_tokens, "temperature": temperature,
            "stream": true,
        });
        if let Some(top_p) = extra.top_p { body["top_p"] = json!(top_p); }
        if let Some(system) = &system_prompt { body["system"] = json!(system); }
        if let Some(tools) = tools {
            if !tools.is_empty() {
                body["tools"] = json!(self.convert_tools(tools));
                let has_tool_results = messages.iter().any(|m| m.role == Role::Tool);
                body["tool_choice"] = if has_tool_results { json!({"type": "auto"}) } else { json!({"type": "any"}) };
            }
        }

        let response = http::client()
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(ProviderError::Api { status: status.as_u16(), message: text });
        }

        let mut content = String::new();
        let mut finish_reason = FinishReason::Stop;
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut current_tool: Option<(String, String, String)> = None; // (id, name, input_json)
        let mut usage = TokenUsage::default();

        let mut stream = response.bytes_stream();
        let mut buf = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| ProviderError::Other(format!("Stream read error: {}", e)))?;
            buf.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(pos) = buf.find('\n') {
                let line = buf[..pos].trim().to_string();
                buf = buf[pos + 1..].to_string();

                if !line.starts_with("data:") { continue; }
                let data = line[5..].trim();
                if data.is_empty() { continue; }

                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                    let event_type = parsed.get("type").and_then(|v| v.as_str()).unwrap_or("");

                    match event_type {
                        "message_start" => {
                            if let Some(u) = parsed.get("message").and_then(|m| m.get("usage")) {
                                usage.prompt_tokens = u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                            }
                        }
                        "content_block_start" => {
                            if let Some(cb) = parsed.get("content_block") {
                                let cb_type = cb.get("type").and_then(|v| v.as_str()).unwrap_or("");
                                if cb_type == "tool_use" {
                                    let id = cb.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                    let name = cb.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                    current_tool = Some((id, name, String::new()));
                                }
                            }
                        }
                        "content_block_delta" => {
                            if let Some(delta) = parsed.get("delta") {
                                let delta_type = delta.get("type").and_then(|v| v.as_str()).unwrap_or("");
                                if delta_type == "text_delta" {
                                    if let Some(text) = delta.get("text").and_then(|v| v.as_str()) {
                                        content.push_str(text);
                                        let _ = chunk_tx.send(text.to_string());
                                    }
                                } else if delta_type == "input_json_delta" {
                                    if let Some(json_str) = delta.get("partial_json").and_then(|v| v.as_str()) {
                                        if let Some(ref mut tool) = current_tool {
                                            tool.2.push_str(json_str);
                                        }
                                    }
                                }
                            }
                        }
                        "content_block_stop" => {
                            if let Some((id, name, args_str)) = current_tool.take() {
                                let arguments: HashMap<String, serde_json::Value> =
                                    serde_json::from_str(&args_str).unwrap_or_else(|_| {
                                        let mut m = HashMap::new();
                                        m.insert("raw".to_string(), serde_json::Value::String(args_str));
                                        m
                                    });
                                tool_calls.push(ToolCall { id, name, arguments });
                            }
                        }
                        "message_delta" => {
                            if let Some(delta) = parsed.get("delta") {
                                if let Some(sr) = delta.get("stop_reason").and_then(|v| v.as_str()) {
                                    finish_reason = match sr {
                                        "end_turn" | "stop" => FinishReason::Stop,
                                        "tool_use" => FinishReason::ToolCalls,
                                        "max_tokens" => FinishReason::Length,
                                        _ => FinishReason::Stop,
                                    };
                                }
                            }
                            if let Some(u) = parsed.get("usage") {
                                usage.completion_tokens = u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                                usage.total_tokens = usage.prompt_tokens + usage.completion_tokens;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        Ok(CompletionResponse {
            content: if content.is_empty() { None } else { Some(content) },
            tool_calls,
            finish_reason,
            usage,
        })
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }
}

impl AnthropicProvider {
    fn parse_response(&self, data: &serde_json::Value) -> Result<CompletionResponse, ProviderError> {
        let content_blocks = data
            .get("content")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ProviderError::Parse("No content in response".to_string()))?;

        let mut text_content = String::new();
        let mut tool_calls = Vec::new();

        for block in content_blocks {
            match block.get("type").and_then(|v| v.as_str()) {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                        text_content.push_str(text);
                    }
                }
                Some("tool_use") => {
                    let id = block
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = block
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let input = block.get("input").cloned().unwrap_or(json!({}));
                    let arguments: HashMap<String, serde_json::Value> =
                        serde_json::from_value(input).unwrap_or_default();

                    tool_calls.push(ToolCall {
                        id,
                        name,
                        arguments,
                    });
                }
                _ => {}
            }
        }

        let finish_reason = match data.get("stop_reason").and_then(|v| v.as_str()) {
            Some("end_turn") => FinishReason::Stop,
            Some("tool_use") => FinishReason::ToolCalls,
            Some("max_tokens") => FinishReason::Length,
            _ => FinishReason::Stop,
        };

        let usage = if let Some(u) = data.get("usage") {
            TokenUsage {
                prompt_tokens: u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                completion_tokens: u
                    .get("output_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32,
                total_tokens: 0, // Anthropic doesn't provide total
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
