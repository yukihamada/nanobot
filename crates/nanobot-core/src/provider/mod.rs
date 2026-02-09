pub mod openai_compat;
pub mod anthropic;
pub mod gemini;

use std::sync::Arc;
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
    providers: Vec<Arc<dyn LlmProvider>>,
    counter: AtomicUsize,
}

impl LoadBalancedProvider {
    pub fn new(providers: Vec<Arc<dyn LlmProvider>>) -> Self {
        Self {
            providers,
            counter: AtomicUsize::new(0),
        }
    }

    /// Create from environment variables. Reads comma-separated API keys.
    pub fn from_env() -> Option<Self> {
        let mut providers: Vec<Arc<dyn LlmProvider>> = Vec::new();

        // OpenAI keys (OPENAI_API_KEY, OPENAI_API_KEY_2, etc.)
        for key in Self::read_keys("OPENAI_API_KEY") {
            providers.push(Arc::new(openai_compat::OpenAiCompatProvider::new(
                key, None, "gpt-4o".to_string(),
            )));
        }

        // Anthropic keys
        for key in Self::read_keys("ANTHROPIC_API_KEY") {
            providers.push(Arc::new(anthropic::AnthropicProvider::new(
                key, None, "claude-sonnet-4-5-20250929".to_string(),
            )));
        }

        // Gemini keys (check both GEMINI_API_KEY and GOOGLE_API_KEY)
        for key in Self::read_keys_multi(&["GEMINI_API_KEY", "GOOGLE_API_KEY"]) {
            providers.push(Arc::new(gemini::GeminiProvider::new(
                key, None, "gemini-2.0-flash".to_string(),
            )));
        }

        // Groq keys (fast inference)
        for key in Self::read_keys("GROQ_API_KEY") {
            providers.push(Arc::new(openai_compat::OpenAiCompatProvider::new(
                key, Some("https://api.groq.com/openai/v1".to_string()), "llama-3.3-70b-versatile".to_string(),
            )));
        }

        // Kimi / Moonshot keys
        for key in Self::read_keys_multi(&["KIMI_API_KEY", "MOONSHOT_API_KEY"]) {
            providers.push(Arc::new(openai_compat::OpenAiCompatProvider::new(
                key, Some("https://api.moonshot.cn/v1".to_string()), "kimi-k2-0711".to_string(),
            )));
        }

        // OpenRouter keys (backup provider — routes to multiple models)
        for key in Self::read_keys("OPENROUTER_API_KEY") {
            providers.push(Arc::new(openai_compat::OpenAiCompatProvider::new(
                key, Some("https://openrouter.ai/api/v1".to_string()), "openrouter/auto".to_string(),
            )));
        }

        if providers.is_empty() {
            None
        } else {
            Some(Self::new(providers))
        }
    }

    fn read_keys(prefix: &str) -> Vec<String> {
        Self::read_keys_multi(&[prefix])
    }

    fn read_keys_multi(prefixes: &[&str]) -> Vec<String> {
        let mut keys = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for prefix in prefixes {
            // Primary key
            if let Ok(key) = std::env::var(prefix) {
                if !key.is_empty() && seen.insert(key.clone()) {
                    keys.push(key);
                }
            }
            // Additional keys: PREFIX_2, PREFIX_3, etc.
            for i in 2..=10 {
                if let Ok(key) = std::env::var(format!("{prefix}_{i}")) {
                    if !key.is_empty() && seen.insert(key.clone()) {
                        keys.push(key);
                    }
                }
            }
        }
        keys
    }

    /// Convert model name to one compatible with the target provider.
    /// When falling back to a different provider family, we must use a model
    /// that provider actually supports.
    fn convert_model_for_provider(provider: &dyn LlmProvider, requested_model: &str) -> String {
        let req_lower = requested_model.to_lowercase();
        let prov_default = provider.default_model().to_lowercase();

        let req_is_claude = req_lower.contains("claude") || req_lower.contains("anthropic");
        let req_is_gpt = req_lower.contains("gpt") || req_lower.contains("openai");
        let req_is_gemini = req_lower.contains("gemini");
        let req_is_groq = req_lower.contains("llama") || req_lower.contains("mixtral") || req_lower.contains("groq");
        let req_is_kimi = req_lower.contains("kimi") || req_lower.contains("moonshot");

        let prov_is_claude = prov_default.contains("claude") || prov_default.contains("anthropic");
        let prov_is_gemini = prov_default.contains("gemini");
        let prov_is_groq = prov_default.contains("llama") || prov_default.contains("mixtral") || prov_default.contains("groq");
        let prov_is_kimi = prov_default.contains("kimi") || prov_default.contains("moonshot");
        let prov_is_openrouter = prov_default.contains("openrouter");
        let prov_is_gpt = !prov_is_claude && !prov_is_gemini && !prov_is_groq && !prov_is_kimi && !prov_is_openrouter;

        // Same family → use requested model as-is
        if (req_is_claude && prov_is_claude)
            || (req_is_gpt && prov_is_gpt)
            || (req_is_gemini && prov_is_gemini)
            || (req_is_groq && prov_is_groq)
            || (req_is_kimi && prov_is_kimi)
        {
            return requested_model.to_string();
        }

        // OpenRouter can handle any model — pass through the requested model
        if prov_is_openrouter {
            return requested_model.to_string();
        }

        // Cross-family → map to the provider's best equivalent
        if prov_is_claude {
            "claude-sonnet-4-5-20250929".to_string()
        } else if prov_is_gemini {
            "gemini-2.0-flash".to_string()
        } else if prov_is_groq {
            "llama-3.3-70b-versatile".to_string()
        } else if prov_is_kimi {
            "kimi-k2-0711".to_string()
        } else {
            // OpenAI-compatible
            "gpt-4o".to_string()
        }
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
                } else if model_lower.contains("llama") || model_lower.contains("mixtral") || model_lower.contains("groq") {
                    default.contains("llama") || default.contains("mixtral") || default.contains("groq")
                } else if model_lower.contains("kimi") || model_lower.contains("moonshot") {
                    default.contains("kimi") || default.contains("moonshot")
                } else if model_lower.contains("gpt") || model_lower.contains("openai") {
                    // Match OpenAI providers but not Groq/Kimi/OpenRouter
                    default.contains("gpt") || (!default.contains("claude") && !default.contains("gemini") && !default.contains("llama") && !default.contains("kimi") && !default.contains("moonshot") && !default.contains("openrouter"))
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

    /// Get list of available models for parallel racing.
    /// Returns (model_name, provider_index) pairs, one per provider family.
    pub fn available_parallel_models(&self) -> Vec<(String, usize)> {
        let mut models = Vec::new();
        let mut seen_families = std::collections::HashSet::new();
        for (i, p) in self.providers.iter().enumerate() {
            let default = p.default_model().to_lowercase();
            let family = if default.contains("claude") { "claude" }
                else if default.contains("gemini") { "gemini" }
                else if default.contains("llama") || default.contains("groq") { "groq" }
                else if default.contains("kimi") || default.contains("moonshot") { "kimi" }
                else if default.contains("openrouter") { continue } // skip openrouter for parallel
                else { "openai" };
            if seen_families.insert(family) {
                models.push((p.default_model().to_string(), i));
            }
        }
        models
    }

    /// Race multiple providers in parallel. Returns the fastest successful response
    /// along with a list of (model, input_tokens, output_tokens) for all completed calls.
    pub async fn chat_parallel(
        &self,
        messages: &[Message],
        tools: Option<&[serde_json::Value]>,
        max_tokens: u32,
        temperature: f64,
    ) -> Result<(CompletionResponse, String, Vec<(String, u32, u32)>), ProviderError> {
        let parallel_models = self.available_parallel_models();
        if parallel_models.is_empty() {
            return Err(ProviderError::Other("No providers available for parallel mode".to_string()));
        }

        let (tx, mut rx) = tokio::sync::mpsc::channel::<(CompletionResponse, String, u32, u32)>(parallel_models.len());
        let msgs = messages.to_vec();
        let tools_owned: Option<Vec<serde_json::Value>> = tools.map(|t| t.to_vec());

        for (model_name, idx) in &parallel_models {
            let provider = self.providers[*idx].clone();
            let model = model_name.clone();
            let msgs = msgs.clone();
            let tools = tools_owned.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                let start = std::time::Instant::now();
                let tools_ref = tools.as_deref();
                match provider.chat(&msgs, tools_ref, &model, max_tokens, temperature).await {
                    Ok(resp) => {
                        let latency = start.elapsed();
                        tracing::info!("Parallel LLM {} responded in {:?}: {} tokens", model, latency, resp.usage.completion_tokens);
                        let _ = tx.send((resp, model, 0, 0)).await; // tokens filled from usage
                    }
                    Err(e) => {
                        tracing::warn!("Parallel LLM {} failed: {}", model, e);
                    }
                }
            });
        }
        drop(tx); // close sender so rx will end when all spawned tasks finish

        // Wait for the first successful response
        if let Some((resp, model, _, _)) = rx.recv().await {
            let input_tokens = resp.usage.prompt_tokens;
            let output_tokens = resp.usage.completion_tokens;
            let mut all_usage = vec![(model.clone(), input_tokens, output_tokens)];

            // Collect remaining results (non-blocking) for credit accounting
            while let Ok((r, m, _, _)) = rx.try_recv() {
                all_usage.push((m, r.usage.prompt_tokens, r.usage.completion_tokens));
            }

            Ok((resp, model, all_usage))
        } else {
            Err(ProviderError::Other("All parallel providers failed".to_string()))
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

        // Failover: try other providers with model name conversion
        for i in 1..total {
            let idx = (start + i) % total;
            let provider = self.providers[idx].as_ref();
            let converted_model = Self::convert_model_for_provider(provider, model);
            tracing::info!("Fallback provider {}: converting model {} -> {}", idx, model, converted_model);
            match provider.chat(messages, tools, &converted_model, max_tokens, temperature).await {
                Ok(resp) => {
                    tracing::info!("Fallback provider {} succeeded with model {}", idx, converted_model);
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
