use async_trait::async_trait;
use serde_json::json;
use std::collections::HashMap;
use tracing::debug;

use crate::error::ProviderError;
use crate::types::{CompletionResponse, FinishReason, Message, TokenUsage, ToolCall};
use crate::util::http;

use super::{LlmProvider, ChatExtra};

/// OpenAI-compatible provider.
/// Works with OpenRouter, DeepSeek, Groq, Moonshot/Kimi, Qwen, MiniMax, vLLM, and any OpenAI-compatible API.
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
            } else if model.contains("qwen") || model.contains("tongyi") {
                "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string()
            } else if model.contains("minimax") {
                "https://api.minimax.chat/v1".to_string()
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

    /// Normalize model name for the API.
    /// OpenRouter requires full model paths (e.g. "minimax/minimax-m2.5").
    /// Native provider APIs require bare model names (e.g. "minimax-m2.5").
    fn normalize_model(&self, model: &str) -> String {
        // OpenRouter needs the full "provider/model" format — do not strip
        if self.api_base.contains("openrouter.ai") {
            return model.to_string();
        }
        // Strip provider prefixes for native APIs
        for prefix in &[
            "openrouter/",
            "openai/",
            "deepseek/",
            "groq/",
            "moonshot/",
            "kimi/",
            "qwen/",
            "tongyi/",
            "minimax/",
            "moonshotai/",
            "z-ai/",
            "google/",
            "anthropic/",
        ] {
            if let Some(rest) = model.strip_prefix(prefix) {
                return rest.to_string();
            }
        }
        model.to_string()
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
                // If this is a follow-up call (has tool results), use "auto".
                // Otherwise force tool usage with "required" so the model actually searches.
                let has_tool_results = messages.iter().any(|m| m.role == crate::types::Role::Tool);
                body["tool_choice"] = if has_tool_results {
                    json!("auto")
                } else {
                    json!("required")
                };
            }
        }

        debug!("OpenAI-compat request to {} with model {}", url, model_name);
        if body.get("tools").is_some() {
            let tc = body.get("tool_choice").map(|v| v.to_string()).unwrap_or_default();
            let num_tools = body.get("tools").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
            tracing::info!("Tools: {} definitions, tool_choice={}", num_tools, tc);
        }

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

        // Get response text for better error reporting
        let response_text = response.text().await?;
        let data: serde_json::Value = serde_json::from_str(&response_text).map_err(|e| {
            tracing::error!("Failed to parse JSON response. Error: {}, Body (first 500 chars): {}", e, &response_text.chars().take(500).collect::<String>());
            ProviderError::Api {
                status: status.as_u16(),
                message: format!("JSON parse error: {}. Body preview: {}", e, &response_text.chars().take(200).collect::<String>()),
            }
        })?;
        parse_openai_response(&data)
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
        let url = format!("{}/chat/completions", self.api_base);
        let model_name = self.normalize_model(model);

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

        // Apply extra parameters
        if let Some(top_p) = extra.top_p {
            body["top_p"] = json!(top_p);
        }
        if let Some(fp) = extra.frequency_penalty {
            body["frequency_penalty"] = json!(fp);
        }
        if let Some(pp) = extra.presence_penalty {
            body["presence_penalty"] = json!(pp);
        }

        if let Some(tools) = tools {
            if !tools.is_empty() {
                body["tools"] = json!(tools);
                let has_tool_results = messages.iter().any(|m| m.role == crate::types::Role::Tool);
                body["tool_choice"] = if has_tool_results {
                    json!("auto")
                } else {
                    json!("required")
                };
            }
        }

        debug!("OpenAI-compat request (with extra) to {} with model {}", url, model_name);

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

        // Get response text for better error reporting
        let response_text = response.text().await?;
        let data: serde_json::Value = serde_json::from_str(&response_text).map_err(|e| {
            tracing::error!("Failed to parse JSON response. Error: {}, Body (first 500 chars): {}", e, &response_text.chars().take(500).collect::<String>());
            ProviderError::Api {
                status: status.as_u16(),
                message: format!("JSON parse error: {}. Body preview: {}", e, &response_text.chars().take(200).collect::<String>()),
            }
        })?;
        parse_openai_response(&data)
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

        let url = format!("{}/chat/completions", self.api_base);
        let model_name = self.normalize_model(model);

        let msgs: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| {
                let mut msg = json!({"role": m.role, "content": m.content.as_deref().unwrap_or("")});
                if let Some(ref tc) = m.tool_calls { msg["tool_calls"] = json!(tc); }
                if let Some(ref id) = m.tool_call_id { msg["tool_call_id"] = json!(id); }
                if let Some(ref name) = m.name { msg["name"] = json!(name); }
                msg
            })
            .collect();

        let mut body = json!({
            "model": model_name, "messages": msgs,
            "max_tokens": max_tokens, "temperature": temperature,
            "stream": true, "stream_options": {"include_usage": true},
        });
        if let Some(top_p) = extra.top_p { body["top_p"] = json!(top_p); }
        if let Some(fp) = extra.frequency_penalty { body["frequency_penalty"] = json!(fp); }
        if let Some(pp) = extra.presence_penalty { body["presence_penalty"] = json!(pp); }
        if let Some(tools) = tools {
            if !tools.is_empty() {
                body["tools"] = json!(tools);
                let has_tool_results = messages.iter().any(|m| m.role == crate::types::Role::Tool);
                body["tool_choice"] = if has_tool_results { json!("auto") } else { json!("required") };
            }
        }

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
            return Err(ProviderError::Api { status: status.as_u16(), message: text });
        }

        // Parse SSE stream
        let mut content = String::new();
        let mut finish_reason = FinishReason::Stop;
        let mut tool_calls_map: std::collections::BTreeMap<usize, (String, String, String)> = std::collections::BTreeMap::new(); // index -> (id, name, args)
        let mut usage = TokenUsage::default();

        let mut stream = response.bytes_stream();
        let mut buf = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| ProviderError::Other(format!("Stream read error: {}", e)))?;
            buf.push_str(&String::from_utf8_lossy(&chunk));

            // Process complete lines
            while let Some(pos) = buf.find('\n') {
                let line = buf[..pos].trim().to_string();
                buf = buf[pos + 1..].to_string();

                if !line.starts_with("data:") { continue; }
                let data = line[5..].trim();
                if data == "[DONE]" { continue; }

                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                    // Usage info (from stream_options.include_usage)
                    if let Some(u) = parsed.get("usage") {
                        usage.prompt_tokens = u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                        usage.completion_tokens = u.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                        usage.total_tokens = u.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                    }

                    if let Some(choice) = parsed.get("choices").and_then(|c| c.get(0)) {
                        // Finish reason
                        if let Some(fr) = choice.get("finish_reason").and_then(|v| v.as_str()) {
                            finish_reason = match fr {
                                "stop" => FinishReason::Stop,
                                "tool_calls" => FinishReason::ToolCalls,
                                "length" => FinishReason::Length,
                                _ => FinishReason::Stop,
                            };
                        }

                        if let Some(delta) = choice.get("delta") {
                            // Content delta
                            if let Some(text) = delta.get("content").and_then(|v| v.as_str()) {
                                content.push_str(text);
                                let _ = chunk_tx.send(text.to_string());
                            }

                            // Tool call deltas
                            if let Some(tcs) = delta.get("tool_calls").and_then(|v| v.as_array()) {
                                for tc in tcs {
                                    let idx = tc.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                                    let entry = tool_calls_map.entry(idx).or_insert_with(|| (String::new(), String::new(), String::new()));
                                    if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
                                        entry.0 = id.to_string();
                                    }
                                    if let Some(f) = tc.get("function") {
                                        if let Some(name) = f.get("name").and_then(|v| v.as_str()) {
                                            entry.1 = name.to_string();
                                        }
                                        if let Some(args) = f.get("arguments").and_then(|v| v.as_str()) {
                                            entry.2.push_str(args);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Build tool_calls from accumulated data
        let tool_calls: Vec<ToolCall> = tool_calls_map.into_values().map(|(id, name, args_str)| {
            let arguments: HashMap<String, serde_json::Value> =
                serde_json::from_str(&args_str).unwrap_or_else(|_| {
                    let mut m = HashMap::new();
                    m.insert("raw".to_string(), serde_json::Value::String(args_str));
                    m
                });
            ToolCall { id, name, arguments }
        }).collect();

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
