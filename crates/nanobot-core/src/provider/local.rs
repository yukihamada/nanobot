//! Local LLM fallback provider using candle for CPU inference.
//!
//! Runs Qwen3-0.6B-Instruct (Q4_K_M GGUF) directly on Lambda ARM64.
//! Used as a last-resort fallback when all remote providers fail.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use async_trait::async_trait;
use candle_core::{quantized::gguf_file, Device, Tensor};
use candle_transformers::generation::LogitsProcessor;
use candle_transformers::models::quantized_qwen2::ModelWeights;
use tokenizers::Tokenizer;

use crate::error::ProviderError;
use crate::types::{CompletionResponse, FinishReason, Message, Role, TokenUsage};

use super::LlmProvider;

/// Loaded model + tokenizer singleton (Lambda = 1 request/instance).
struct LoadedModel {
    model: ModelWeights,
    tokenizer: Tokenizer,
    device: Device,
}

static MODEL: OnceLock<Mutex<LoadedModel>> = OnceLock::new();

/// Local LLM provider for fallback inference.
pub struct LocalProvider {
    model_url: String,
    tokenizer_url: String,
}

impl LocalProvider {
    /// Create from environment variables.
    /// Returns None if LOCAL_MODEL_URL is not set.
    pub fn from_env() -> Option<Self> {
        let model_url = std::env::var("LOCAL_MODEL_URL").ok()?;
        let tokenizer_url = std::env::var("LOCAL_TOKENIZER_URL")
            .unwrap_or_else(|_| String::new());
        if model_url.is_empty() {
            return None;
        }
        Some(Self {
            model_url,
            tokenizer_url,
        })
    }

    /// Check if the local model is loaded in memory.
    pub fn is_loaded() -> bool {
        MODEL.get().is_some()
    }

    /// Check if a local model is configured (env vars set).
    pub fn is_configured() -> bool {
        std::env::var("LOCAL_MODEL_URL")
            .map(|v| !v.is_empty())
            .unwrap_or(false)
    }

    /// Estimate memory usage of the loaded model in MB.
    /// Qwen3-0.6B Q4_K_M is approximately 350-400 MB.
    pub fn estimated_memory_mb() -> u64 {
        if Self::is_loaded() { 350 } else { 0 }
    }

    /// Download a file from URL to /tmp if not already cached.
    async fn download_to_tmp(url: &str, filename: &str) -> Result<PathBuf, ProviderError> {
        let path = PathBuf::from("/tmp").join(filename);
        if path.exists() {
            tracing::info!("Local model file already cached: {}", path.display());
            return Ok(path);
        }

        tracing::info!("Downloading local model file: {} -> {}", url, path.display());
        let resp = reqwest::get(url).await.map_err(|e| {
            ProviderError::Other(format!("Failed to download model: {}", e))
        })?;
        if !resp.status().is_success() {
            return Err(ProviderError::Other(format!(
                "Model download HTTP {}: {}",
                resp.status(),
                url
            )));
        }
        let bytes = resp.bytes().await.map_err(|e| {
            ProviderError::Other(format!("Failed to read model bytes: {}", e))
        })?;
        tokio::fs::write(&path, &bytes).await.map_err(|e| {
            ProviderError::Other(format!("Failed to write model to /tmp: {}", e))
        })?;
        tracing::info!("Downloaded {} bytes to {}", bytes.len(), path.display());
        Ok(path)
    }

    /// Ensure the model is loaded into memory (lazy singleton).
    async fn ensure_loaded(&self) -> Result<(), ProviderError> {
        if MODEL.get().is_some() {
            return Ok(());
        }

        let model_path = Self::download_to_tmp(&self.model_url, "local-model.gguf").await?;

        let tokenizer = if self.tokenizer_url.is_empty() {
            // If no tokenizer URL, try loading from HuggingFace Hub
            tracing::info!("Loading tokenizer from HuggingFace Hub for Qwen3-0.6B");
            let api = hf_hub::api::tokio::Api::new().map_err(|e| {
                ProviderError::Other(format!("HF Hub API error: {}", e))
            })?;
            let repo = api.model("Qwen/Qwen3-0.6B-Instruct".to_string());
            let tokenizer_path = repo.get("tokenizer.json").await.map_err(|e| {
                ProviderError::Other(format!("Failed to download tokenizer: {}", e))
            })?;
            Tokenizer::from_file(tokenizer_path).map_err(|e| {
                ProviderError::Other(format!("Failed to load tokenizer: {}", e))
            })?
        } else {
            let tokenizer_path =
                Self::download_to_tmp(&self.tokenizer_url, "tokenizer.json").await?;
            Tokenizer::from_file(tokenizer_path).map_err(|e| {
                ProviderError::Other(format!("Failed to load tokenizer: {}", e))
            })?
        };

        // Load GGUF model on CPU (blocking operation)
        let model_path_clone = model_path.clone();
        let loaded = tokio::task::spawn_blocking(move || {
            let device = Device::Cpu;
            let mut file = std::fs::File::open(&model_path_clone).map_err(|e| {
                ProviderError::Other(format!("Failed to open model file: {}", e))
            })?;
            let content = gguf_file::Content::read(&mut file).map_err(|e| {
                ProviderError::Other(format!("Failed to read GGUF content: {}", e))
            })?;
            let model = ModelWeights::from_gguf(content, &mut file, &device)
                .map_err(|e| ProviderError::Other(format!("Failed to load GGUF model: {}", e)))?;

            Ok::<_, ProviderError>((model, device))
        })
        .await
        .map_err(|e| ProviderError::Other(format!("spawn_blocking join error: {}", e)))??;

        let _ = MODEL.set(Mutex::new(LoadedModel {
            model: loaded.0,
            tokenizer,
            device: loaded.1,
        }));

        tracing::info!("Local model loaded successfully");
        Ok(())
    }

    /// Format messages into ChatML format for Qwen3.
    fn format_chatml(messages: &[Message]) -> String {
        let mut prompt = String::new();
        for msg in messages {
            let role = match msg.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            };
            let content = msg.content.as_deref().unwrap_or("");
            prompt.push_str(&format!("<|im_start|>{}\n{}<|im_end|>\n", role, content));
        }
        // Add generation prompt — disable thinking mode for speed
        prompt.push_str("<|im_start|>assistant\n<|im_start|>think\n\n<|im_end|>\n");
        prompt
    }

    /// Run autoregressive text generation.
    fn generate(
        model: &mut ModelWeights,
        tokenizer: &Tokenizer,
        device: &Device,
        prompt: &str,
        max_tokens: u32,
        temperature: f64,
    ) -> Result<(String, u32, u32), ProviderError> {
        let encoding = tokenizer.encode(prompt, true).map_err(|e| {
            ProviderError::Other(format!("Tokenizer encode error: {}", e))
        })?;
        let input_ids = encoding.get_ids();
        let prompt_tokens = input_ids.len() as u32;

        let mut logits_processor = LogitsProcessor::new(
            42, // seed
            Some(temperature),
            Some(0.9), // top_p
        );

        // Get EOS token ID for <|im_end|>
        let eos_token = tokenizer
            .token_to_id("<|im_end|>")
            .unwrap_or(151645); // Qwen3 default EOS

        let mut generated_tokens: Vec<u32> = Vec::new();
        let max_gen = max_tokens.min(512) as usize; // cap at 512 for speed

        // Prefill: feed the full prompt through the model
        let input_tensor = Tensor::new(input_ids, device).map_err(|e| {
            ProviderError::Other(format!("Tensor creation error: {}", e))
        })?;
        let input_tensor = input_tensor.unsqueeze(0).map_err(|e| {
            ProviderError::Other(format!("Unsqueeze error: {}", e))
        })?;
        let logits = model.forward(&input_tensor, 0).map_err(|e| {
            ProviderError::Other(format!("Model forward error: {}", e))
        })?;
        let logits = logits.squeeze(0).map_err(|e| {
            ProviderError::Other(format!("Squeeze error: {}", e))
        })?;

        // Get logits for the last position
        let seq_len = logits.dim(0).map_err(|e| {
            ProviderError::Other(format!("Dim error: {}", e))
        })?;
        let last_logits = logits.get(seq_len - 1).map_err(|e| {
            ProviderError::Other(format!("Get last logits error: {}", e))
        })?;

        let mut current_token = logits_processor.sample(&last_logits).map_err(|e| {
            ProviderError::Other(format!("Sampling error: {}", e))
        })?;

        if current_token == eos_token {
            let text = tokenizer.decode(&generated_tokens, true).unwrap_or_default();
            return Ok((text, prompt_tokens, 0));
        }
        generated_tokens.push(current_token);

        // Decode: autoregressive generation one token at a time
        for i in 1..max_gen {
            let pos = prompt_tokens as usize + i;
            let input = Tensor::new(&[current_token], device).map_err(|e| {
                ProviderError::Other(format!("Tensor error: {}", e))
            })?;
            let input = input.unsqueeze(0).map_err(|e| {
                ProviderError::Other(format!("Unsqueeze error: {}", e))
            })?;
            let logits = model.forward(&input, pos).map_err(|e| {
                ProviderError::Other(format!("Forward error at pos {}: {}", pos, e))
            })?;
            let logits = logits.squeeze(0).map_err(|e| {
                ProviderError::Other(format!("Squeeze error: {}", e))
            })?;
            let logits_last = logits.get(0).map_err(|e| {
                ProviderError::Other(format!("Get error: {}", e))
            })?;
            let next_id = logits_processor.sample(&logits_last).map_err(|e| {
                ProviderError::Other(format!("Sample error: {}", e))
            })?;

            if next_id == eos_token {
                break;
            }
            generated_tokens.push(next_id);
            current_token = next_id;
        }

        let completion_tokens = generated_tokens.len() as u32;
        let text = tokenizer.decode(&generated_tokens, true).unwrap_or_default();

        Ok((text, prompt_tokens, completion_tokens))
    }
}

#[async_trait]
impl LlmProvider for LocalProvider {
    async fn chat(
        &self,
        messages: &[Message],
        _tools: Option<&[serde_json::Value]>,
        _model: &str,
        max_tokens: u32,
        temperature: f64,
    ) -> Result<CompletionResponse, ProviderError> {
        // Ensure model is loaded
        self.ensure_loaded().await?;

        let prompt = Self::format_chatml(messages);
        let max_tokens = max_tokens.min(512); // Cap for speed on CPU
        let temperature = if temperature < 0.01 { 0.6 } else { temperature };

        // Run inference in a blocking task
        let (text, prompt_tokens, completion_tokens) =
            tokio::task::spawn_blocking(move || {
                let guard = MODEL
                    .get()
                    .ok_or_else(|| ProviderError::Other("Model not loaded".to_string()))?;
                let mut loaded = guard.lock().map_err(|e| {
                    ProviderError::Other(format!("Model lock poisoned: {}", e))
                })?;
                let LoadedModel { ref mut model, ref tokenizer, ref device } = *loaded;
                Self::generate(
                    model,
                    tokenizer,
                    device,
                    &prompt,
                    max_tokens,
                    temperature,
                )
            })
            .await
            .map_err(|e| ProviderError::Other(format!("Inference task error: {}", e)))??;

        // Add fallback disclaimer
        let response_text = format!(
            "{}\n\n---\n⚡ ローカルフォールバックモデル (Qwen3-0.6B) による応答です",
            text.trim()
        );

        Ok(CompletionResponse {
            content: Some(response_text),
            tool_calls: vec![],
            finish_reason: FinishReason::Stop,
            usage: TokenUsage {
                prompt_tokens,
                completion_tokens,
                total_tokens: prompt_tokens + completion_tokens,
            },
        })
    }

    fn default_model(&self) -> &str {
        "local-qwen3-0.6b"
    }
}
