pub mod openai_compat;
pub mod anthropic;
pub mod gemini;

use std::sync::atomic::{AtomicUsize, Ordering};
use async_trait::async_trait;

use crate::error::ProviderError;
use crate::types::{CompletionResponse, Message};

/// Trait for LLM providers.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Send a chat completion request.
    async fn chat(
        &self,
        messages: &[Message],
        tools: Option<&[serde_json::Value]>,
        model: &str,
        max_tokens: u32,
        temperature: f64,
    ) -> Result<CompletionResponse, ProviderError>;

    /// Get the default model for this provider.
    fn default_model(&self) -> &str;
}

/// Create the appropriate provider based on model name and config.
pub fn create_provider(
    api_key: &str,
    api_base: Option<&str>,
    default_model: &str,
) -> Box<dyn LlmProvider> {
    let model_lower = default_model.to_lowercase();

    // Use native Anthropic provider for Anthropic models (unless via OpenRouter)
    if (model_lower.contains("anthropic") || model_lower.contains("claude"))
        && !model_lower.contains("openrouter")
    {
        return Box::new(anthropic::AnthropicProvider::new(
            api_key.to_string(),
            api_base.map(|s| s.to_string()),
            default_model.to_string(),
        ));
    }

    // Use native Gemini provider
    if model_lower.contains("gemini") && !model_lower.contains("openrouter") {
        return Box::new(gemini::GeminiProvider::new(
            api_key.to_string(),
            api_base.map(|s| s.to_string()),
            default_model.to_string(),
        ));
    }

    // Default: OpenAI-compatible provider (works with OpenRouter, DeepSeek, Groq, etc.)
    Box::new(openai_compat::OpenAiCompatProvider::new(
        api_key.to_string(),
        api_base.map(|s| s.to_string()),
        default_model.to_string(),
    ))
}

/// Load-balanced provider that distributes requests across multiple providers
/// with automatic failover.
pub struct LoadBalancedProvider {
    providers: Vec<Box<dyn LlmProvider>>,
    counter: AtomicUsize,
}

impl LoadBalancedProvider {
    pub fn new(providers: Vec<Box<dyn LlmProvider>>) -> Self {
        Self {
            providers,
            counter: AtomicUsize::new(0),
        }
    }

    /// Create from environment variables. Reads comma-separated API keys.
    pub fn from_env() -> Option<Self> {
        let mut providers: Vec<Box<dyn LlmProvider>> = Vec::new();

        // OpenAI keys (OPENAI_API_KEY, OPENAI_API_KEY_2, etc.)
        for key in Self::read_keys("OPENAI_API_KEY") {
            providers.push(Box::new(openai_compat::OpenAiCompatProvider::new(
                key, None, "gpt-4o".to_string(),
            )));
        }

        // Anthropic keys
        for key in Self::read_keys("ANTHROPIC_API_KEY") {
            providers.push(Box::new(anthropic::AnthropicProvider::new(
                key, None, "claude-sonnet-4-5-20250929".to_string(),
            )));
        }

        // Gemini keys
        for key in Self::read_keys("GEMINI_API_KEY") {
            providers.push(Box::new(gemini::GeminiProvider::new(
                key, None, "gemini-2.0-flash".to_string(),
            )));
        }

        if providers.is_empty() {
            None
        } else {
            Some(Self::new(providers))
        }
    }

    fn read_keys(prefix: &str) -> Vec<String> {
        let mut keys = Vec::new();
        // Primary key
        if let Ok(key) = std::env::var(prefix) {
            if !key.is_empty() {
                keys.push(key);
            }
        }
        // Additional keys: PREFIX_2, PREFIX_3, etc.
        for i in 2..=10 {
            if let Ok(key) = std::env::var(format!("{}_{}", prefix, i)) {
                if !key.is_empty() {
                    keys.push(key);
                }
            }
        }
        keys
    }

    /// Select the best provider for a given model.
    fn select_provider(&self, model: &str) -> &dyn LlmProvider {
        let model_lower = model.to_lowercase();

        // Filter providers that match the model
        let matching: Vec<usize> = self.providers.iter().enumerate()
            .filter(|(_, p)| {
                let default = p.default_model().to_lowercase();
                if model_lower.contains("claude") || model_lower.contains("anthropic") {
                    default.contains("claude") || default.contains("anthropic")
                } else if model_lower.contains("gemini") {
                    default.contains("gemini")
                } else if model_lower.contains("gpt") || model_lower.contains("openai") {
                    default.contains("gpt") || !default.contains("claude") && !default.contains("gemini")
                } else {
                    true
                }
            })
            .map(|(i, _)| i)
            .collect();

        if matching.is_empty() {
            // Fallback to round-robin across all providers
            let idx = self.counter.fetch_add(1, Ordering::Relaxed) % self.providers.len();
            self.providers[idx].as_ref()
        } else {
            // Round-robin among matching providers
            let idx = self.counter.fetch_add(1, Ordering::Relaxed) % matching.len();
            self.providers[matching[idx]].as_ref()
        }
    }
}

#[async_trait]
impl LlmProvider for LoadBalancedProvider {
    async fn chat(
        &self,
        messages: &[Message],
        tools: Option<&[serde_json::Value]>,
        model: &str,
        max_tokens: u32,
        temperature: f64,
    ) -> Result<CompletionResponse, ProviderError> {
        let total = self.providers.len();
        let start = self.counter.load(Ordering::Relaxed);

        // Try with matching provider first, then failover to others
        let primary = self.select_provider(model);
        match primary.chat(messages, tools, model, max_tokens, temperature).await {
            Ok(resp) => return Ok(resp),
            Err(e) => {
                tracing::warn!("Primary provider failed for model {}: {}, trying fallback", model, e);
            }
        }

        // Failover: try other providers
        for i in 1..total {
            let idx = (start + i) % total;
            let provider = self.providers[idx].as_ref();
            match provider.chat(messages, tools, model, max_tokens, temperature).await {
                Ok(resp) => {
                    tracing::info!("Fallback provider {} succeeded for model {}", idx, model);
                    return Ok(resp);
                }
                Err(e) => {
                    tracing::warn!("Fallback provider {} failed: {}", idx, e);
                }
            }
        }

        Err(ProviderError::Other("All providers failed".to_string()))
    }

    fn default_model(&self) -> &str {
        self.providers.first().map(|p| p.default_model()).unwrap_or("gpt-4o")
    }
}
