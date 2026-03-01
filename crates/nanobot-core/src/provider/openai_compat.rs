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
    fn is_runpod(&self) -> bool {
        self.api_base.contains("runpod.net") || self.api_base.contains("proxy.runpod")
    }

    /// Strip Nemotron thinking content that appears before `</think>`.
    /// Nemotron-H outputs English reasoning before `</think>`, then the actual Japanese answer.
    fn strip_think(content: String) -> String {
        if let Some(pos) = content.find("</think>") {
            content[pos + 8..].trim_start().to_string()
        } else {
            content
        }
    }

    /// Parse Nemotron's `<TOOLCALL>[{"name":...,"arguments":{...}}]</TOOLCALL>` format.
    /// Returns (tool_calls, remaining_content) — remaining content is empty if all content was tool calls.
    fn parse_toolcall_format(content: &str) -> (Vec<ToolCall>, Option<String>) {
        let start = match content.find("<TOOLCALL>") {
            Some(i) => i,
            None => return (vec![], Some(content.trim().to_string()).filter(|s| !s.is_empty())),
        };
        let end = match content.find("</TOOLCALL>") {
            Some(i) => i,
            None => return (vec![], Some(content.trim().to_string()).filter(|s| !s.is_empty())),
        };

        let json_str = &content[start + 10..end]; // "<TOOLCALL>" = 10 chars
        let before = content[..start].trim();
        let after = content[end + 11..].trim(); // "</TOOLCALL>" = 11 chars
        let remaining = format!("{} {}", before, after).trim().to_string();

        let tool_calls: Vec<ToolCall> = match serde_json::from_str::<serde_json::Value>(json_str) {
            Ok(arr) if arr.is_array() => arr.as_array().unwrap().iter().filter_map(|tc| {
                let name = tc.get("name")?.as_str()?.to_string();
                let args = tc.get("arguments").cloned().unwrap_or(serde_json::json!({}));
                let arguments: HashMap<String, serde_json::Value> = if let Some(obj) = args.as_object() {
                    obj.clone().into_iter().collect()
                } else {
                    let mut m = HashMap::new();
                    m.insert("raw".to_string(), args);
                    m
                };
                Some(ToolCall { id: format!("call_{}", &name), name, arguments })
            }).collect(),
            _ => return (vec![], Some(remaining).filter(|s| !s.is_empty())),
        };

        if tool_calls.is_empty() {
            (vec![], Some(remaining).filter(|s| !s.is_empty()))
        } else {
            (tool_calls, Some(remaining).filter(|s| !s.is_empty()))
        }
    }

    /// For vLLM pods, extract available output tokens from a max_tokens error.
    /// Error format: "... (8192 > 8192 - 3001) ..." → returns 8192 - 3001 - 10 = 5181
    fn extract_available_tokens_from_error(error_msg: &str) -> Option<u32> {
        let gt_pos = error_msg.rfind(" > ")?;
        let right = &error_msg[gt_pos + 3..];
        let minus_pos = right.find(" - ")?;
        let max_model_len: u32 = right[..minus_pos].trim().parse().ok()?;
        let end = right[minus_pos + 3..]
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(right.len().saturating_sub(minus_pos + 3));
        let input_tokens: u32 = right[minus_pos + 3..minus_pos + 3 + end].trim().parse().ok()?;
        let available = max_model_len.saturating_sub(input_tokens);
        if available > 50 { Some(available.saturating_sub(10)) } else { None }
    }

    /// For RunPod vLLM pods, omit max_tokens entirely — vLLM auto-calculates remaining space.
    /// For other providers, return the requested value as-is.
    fn runpod_max_tokens(&self, max_tokens: u32) -> Option<u32> {
        if self.is_runpod() { None } else { Some(max_tokens) }
    }

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
        // vLLM on RunPod GPU Pod — model ID must match exactly (e.g. nvidia/NVIDIA-Nemotron-...)
        if self.api_base.contains("runpod.net") || self.api_base.contains("proxy.runpod") {
            return model.to_string();
        }
        // DeepInfra uses full HuggingFace-style paths (e.g. "nvidia/NVIDIA-Nemotron-Nano-9B-v2-Japanese")
        if self.api_base.contains("deepinfra.com") {
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
            "nvidia/",
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
            "temperature": temperature,
        });
        // RunPod: omit max_tokens — vLLM auto-calculates remaining space (avoids 400 on long contexts)
        if let Some(mt) = self.runpod_max_tokens(max_tokens) { body["max_tokens"] = json!(mt); }

        // RunPod/Nemotron: disable thinking mode — saves 200-800 tokens (~6-20s latency)
        // vLLM supports chat_template_kwargs to toggle thinking per-request.
        if self.is_runpod() {
            body["chat_template_kwargs"] = json!({"enable_thinking": false});
        }

        if let Some(tools) = tools {
            if !tools.is_empty() {
                body["tools"] = json!(tools);
                // RunPod: vLLM launched with --enable-auto-tool-choice --tool-call-parser hermes.
                // Use "auto" (never "required") — Nemotron-9B may not always produce valid tool calls.
                let has_tool_results = messages.iter().any(|m| m.role == crate::types::Role::Tool);
                body["tool_choice"] = if self.is_runpod() || has_tool_results {
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
            // RunPod/vLLM: retry with exact available tokens if max_tokens exceeded
            if self.is_runpod() && status.as_u16() == 400
                && (text.contains("max_tokens") || text.contains("max_completion_tokens"))
            {
                if let Some(reduced) = Self::extract_available_tokens_from_error(&text) {
                    tracing::warn!("RunPod max_tokens too large, retrying with {}", reduced);
                    body["max_tokens"] = json!(reduced);
                    let retry = http::client()
                        .post(&url)
                        .header("Authorization", format!("Bearer {}", self.api_key))
                        .header("Content-Type", "application/json")
                        .json(&body)
                        .send()
                        .await?;
                    let rs = retry.status();
                    if !rs.is_success() {
                        let rt = retry.text().await.unwrap_or_default();
                        return Err(ProviderError::Api { status: rs.as_u16(), message: rt });
                    }
                    let rt = retry.text().await?;
                    let data: serde_json::Value = serde_json::from_str(&rt)
                        .map_err(|e| ProviderError::Api { status: 200, message: format!("JSON: {}", e) })?;
                    let mut resp = parse_openai_response(&data)?;
                    if let Some(c) = resp.content.take() {
                        let stripped = Self::strip_think(c);
                        if stripped.contains("<TOOLCALL>") {
                            let (tc, rem) = Self::parse_toolcall_format(&stripped);
                            if !tc.is_empty() {
                                resp.tool_calls = tc; resp.finish_reason = FinishReason::ToolCalls; resp.content = rem;
                                return Ok(resp);
                            }
                        }
                        resp.content = Some(stripped).filter(|s| !s.is_empty());
                    }
                    return Ok(resp);
                }
            }
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
        let mut resp = parse_openai_response(&data)?;
        if self.is_runpod() {
            if let Some(c) = resp.content.take() {
                let stripped = Self::strip_think(c);
                // Parse Nemotron's <TOOLCALL> format if present
                if stripped.contains("<TOOLCALL>") {
                    let (tool_calls, remaining) = Self::parse_toolcall_format(&stripped);
                    if !tool_calls.is_empty() {
                        resp.tool_calls = tool_calls;
                        resp.finish_reason = FinishReason::ToolCalls;
                        resp.content = remaining;
                        return Ok(resp);
                    }
                }
                resp.content = Some(stripped).filter(|s| !s.is_empty());
            }
        }
        Ok(resp)
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
            "temperature": temperature,
        });
        if let Some(mt) = self.runpod_max_tokens(max_tokens) { body["max_tokens"] = json!(mt); }

        // RunPod/Nemotron: disable thinking mode — saves 200-800 tokens (~6-20s latency)
        if self.is_runpod() {
            body["chat_template_kwargs"] = json!({"enable_thinking": false});
        }

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

        if !self.is_runpod() {
            if let Some(tools) = tools {
                if !tools.is_empty() {
                    body["tools"] = json!(tools);
                    let has_tool_results = messages.iter().any(|m| m.role == crate::types::Role::Tool);
                    body["tool_choice"] = if self.is_runpod() || has_tool_results {
                        json!("auto")
                    } else {
                        json!("required")
                    };
                }
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
            // RunPod/vLLM: retry with exact available tokens if max_tokens exceeded
            if self.is_runpod() && status.as_u16() == 400
                && (text.contains("max_tokens") || text.contains("max_completion_tokens"))
            {
                if let Some(reduced) = Self::extract_available_tokens_from_error(&text) {
                    tracing::warn!("RunPod max_tokens too large (extra), retrying with {}", reduced);
                    body["max_tokens"] = json!(reduced);
                    let retry = http::client()
                        .post(&url)
                        .header("Authorization", format!("Bearer {}", self.api_key))
                        .header("Content-Type", "application/json")
                        .json(&body)
                        .send()
                        .await?;
                    let rs = retry.status();
                    if !rs.is_success() {
                        let rt = retry.text().await.unwrap_or_default();
                        return Err(ProviderError::Api { status: rs.as_u16(), message: rt });
                    }
                    let rt = retry.text().await?;
                    let data: serde_json::Value = serde_json::from_str(&rt)
                        .map_err(|e| ProviderError::Api { status: 200, message: format!("JSON: {}", e) })?;
                    let mut resp = parse_openai_response(&data)?;
                    if self.is_runpod() {
                        if let Some(c) = resp.content.take() {
                            let stripped = Self::strip_think(c);
                            if stripped.contains("<TOOLCALL>") {
                                let (tc, rem) = Self::parse_toolcall_format(&stripped);
                                if !tc.is_empty() {
                                    resp.tool_calls = tc; resp.finish_reason = FinishReason::ToolCalls; resp.content = rem;
                                    return Ok(resp);
                                }
                            }
                            resp.content = Some(stripped).filter(|s| !s.is_empty());
                        }
                    }
                    return Ok(resp);
                }
            }
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
        let mut resp = parse_openai_response(&data)?;
        if self.is_runpod() {
            if let Some(c) = resp.content.take() {
                let stripped = Self::strip_think(c);
                if stripped.contains("<TOOLCALL>") {
                    let (tool_calls, remaining) = Self::parse_toolcall_format(&stripped);
                    if !tool_calls.is_empty() {
                        resp.tool_calls = tool_calls;
                        resp.finish_reason = FinishReason::ToolCalls;
                        resp.content = remaining;
                        return Ok(resp);
                    }
                }
                resp.content = Some(stripped).filter(|s| !s.is_empty());
            }
        }
        Ok(resp)
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
            "temperature": temperature,
            "stream": true, "stream_options": {"include_usage": true},
        });
        if let Some(mt) = self.runpod_max_tokens(max_tokens) { body["max_tokens"] = json!(mt); }
        // RunPod/Nemotron: disable thinking mode — saves 200-800 tokens (~6-20s latency)
        if self.is_runpod() {
            body["chat_template_kwargs"] = json!({"enable_thinking": false});
        }
        if let Some(top_p) = extra.top_p { body["top_p"] = json!(top_p); }
        if let Some(fp) = extra.frequency_penalty { body["frequency_penalty"] = json!(fp); }
        if let Some(pp) = extra.presence_penalty { body["presence_penalty"] = json!(pp); }
        if !self.is_runpod() {
            if let Some(tools) = tools {
                if !tools.is_empty() {
                    body["tools"] = json!(tools);
                    let has_tool_results = messages.iter().any(|m| m.role == crate::types::Role::Tool);
                    body["tool_choice"] = if self.is_runpod() || has_tool_results { json!("auto") } else { json!("required") };
                }
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

        // enable_thinking: false is sent to RunPod, so no </think> tag appears.
        // Always forward content directly (think_done = true).
        let mut think_done = true;
        let mut think_buf = String::new();

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
                                if think_done {
                                    content.push_str(text);
                                    let _ = chunk_tx.send(text.to_string());
                                } else {
                                    // Buffer until we find </think>
                                    think_buf.push_str(text);
                                    if let Some(pos) = think_buf.find("</think>") {
                                        think_done = true;
                                        let after = think_buf[pos + 8..].trim_start().to_string();
                                        think_buf.clear();
                                        if !after.is_empty() {
                                            content.push_str(&after);
                                            let _ = chunk_tx.send(after);
                                        }
                                    }
                                }
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
