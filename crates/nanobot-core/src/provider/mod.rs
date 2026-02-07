pub mod openai_compat;
pub mod anthropic;
pub mod gemini;

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
