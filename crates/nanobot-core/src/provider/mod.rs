pub mod openai_compat;
pub mod anthropic;
pub mod gemini;
pub mod pricing;
pub mod embeddings;
#[cfg(feature = "local-fallback")]
pub mod local;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use async_trait::async_trait;

use crate::error::ProviderError;
use crate::types::{CompletionResponse, Message};

/// Extra LLM parameters beyond temperature/max_tokens.
#[derive(Debug, Clone, Default)]
pub struct ChatExtra {
    pub top_p: Option<f64>,
    pub frequency_penalty: Option<f64>,
    pub presence_penalty: Option<f64>,
}

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

    /// Send a chat completion request with extra parameters.
    /// Default implementation ignores extra params and delegates to chat().
    async fn chat_with_extra(
        &self,
        messages: &[Message],
        tools: Option<&[serde_json::Value]>,
        model: &str,
        max_tokens: u32,
        temperature: f64,
        _extra: &ChatExtra,
    ) -> Result<CompletionResponse, ProviderError> {
        self.chat(messages, tools, model, max_tokens, temperature).await
    }

    /// Stream a chat completion, sending content deltas through `chunk_tx`.
    /// Returns the full CompletionResponse (with accumulated content + tool_calls).
    /// Default: falls back to non-streaming chat_with_extra and sends full content as one chunk.
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
        let resp = self.chat_with_extra(messages, tools, model, max_tokens, temperature, extra).await?;
        if let Some(ref content) = resp.content {
            let _ = chunk_tx.send(content.clone());
        }
        Ok(resp)
    }

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

/// Circuit breaker cooldown: 5 minutes after 3 consecutive failures.
const CIRCUIT_BREAKER_THRESHOLD: u32 = 3;
const CIRCUIT_BREAKER_COOLDOWN_SECS: u64 = 300;

/// Load-balanced provider that distributes requests across multiple providers
/// with automatic failover and per-provider circuit breakers.
pub struct LoadBalancedProvider {
    providers: Vec<Arc<dyn LlmProvider>>,
    counter: AtomicUsize,
    /// Consecutive failure counts per provider index.
    failure_counts: Vec<AtomicU32>,
    /// Unix timestamp (seconds) until which each provider's circuit is open (0 = closed).
    circuit_open_until: Vec<AtomicU64>,
}

impl LoadBalancedProvider {
    pub fn new(providers: Vec<Arc<dyn LlmProvider>>) -> Self {
        let n = providers.len();
        Self {
            providers,
            counter: AtomicUsize::new(0),
            failure_counts: (0..n).map(|_| AtomicU32::new(0)).collect(),
            circuit_open_until: (0..n).map(|_| AtomicU64::new(0)).collect(),
        }
    }

    /// Returns true if the provider at `idx` is currently available (circuit closed).
    fn is_provider_available(&self, idx: usize) -> bool {
        let open_until = self.circuit_open_until[idx].load(Ordering::Relaxed);
        if open_until == 0 {
            return true;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if now >= open_until {
            // Cooldown expired — reset and close circuit
            self.circuit_open_until[idx].store(0, Ordering::Relaxed);
            self.failure_counts[idx].store(0, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    /// Record a failure for the provider at `idx`. Opens the circuit after threshold.
    /// Only counts server errors (5xx) — client errors (4xx) are not the provider's fault.
    pub fn record_failure(&self, idx: usize) {
        if idx >= self.providers.len() { return; }
        let count = self.failure_counts[idx].fetch_add(1, Ordering::Relaxed) + 1;
        if count >= CIRCUIT_BREAKER_THRESHOLD {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let open_until = now + CIRCUIT_BREAKER_COOLDOWN_SECS;
            self.circuit_open_until[idx].store(open_until, Ordering::Relaxed);
            tracing::warn!(
                "Circuit breaker OPEN for provider #{} ({}) — {} failures, cooling down {}s",
                idx, self.providers[idx].default_model(), count, CIRCUIT_BREAKER_COOLDOWN_SECS
            );
        }
    }

    /// Record a failure only if it's a server-side error (5xx / network / timeout).
    /// Client errors (4xx) like invalid model names don't trigger the circuit breaker.
    pub fn record_failure_if_server_error(&self, idx: usize, err: &crate::error::ProviderError) {
        match err {
            crate::error::ProviderError::Api { status, .. } if *status < 500 => {
                tracing::debug!(
                    "Provider #{} returned client error ({}), NOT triggering circuit breaker",
                    idx, status
                );
            }
            _ => {
                self.record_failure(idx);
            }
        }
    }

    /// Record a success for the provider at `idx` — resets failure count.
    pub fn record_success(&self, idx: usize) {
        if idx >= self.providers.len() { return; }
        self.failure_counts[idx].store(0, Ordering::Relaxed);
    }

    /// Returns true if ALL providers have open circuits (none available).
    pub fn all_providers_down(&self) -> bool {
        if self.providers.is_empty() { return true; }
        (0..self.providers.len()).all(|i| !self.is_provider_available(i))
    }

    /// Select the best available provider index for a given model.
    /// Skips providers whose circuit is open.
    fn select_provider_idx(&self, model: &str) -> usize {
        let model_lower = model.to_lowercase();

        // Collect indices of available matching providers
        let matching: Vec<usize> = self.providers.iter().enumerate()
            .filter(|(i, p)| {
                if !self.is_provider_available(*i) { return false; }
                let default = p.default_model().to_lowercase();
                if model_lower.contains("claude") || model_lower.contains("anthropic") {
                    default.contains("claude") || default.contains("anthropic")
                } else if model_lower.contains("gemini") {
                    default.contains("gemini")
                } else if model_lower.contains("kimi") || model_lower.contains("moonshot") {
                    default.contains("kimi") || default.contains("moonshot")
                } else if model_lower.contains("qwen") {
                    default.contains("qwen")
                } else if model_lower.contains("llama") || model_lower.contains("mixtral") || model_lower.contains("groq") {
                    default.contains("llama") || default.contains("mixtral") || default.contains("groq")
                } else if model_lower.contains("deepseek") {
                    default.contains("deepseek")
                } else if model_lower.contains("minimax") || model_lower.contains("m2.5") {
                    default.contains("minimax")
                } else if model_lower.contains("glm") || model_lower.contains("z-ai") {
                    default.contains("glm") || default.contains("z-ai")
                } else if model_lower.contains("gpt") || model_lower.contains("openai") {
                    default.contains("gpt") || (!default.contains("claude") && !default.contains("gemini") && !default.contains("llama") && !default.contains("deepseek") && !default.contains("openrouter") && !default.contains("kimi") && !default.contains("minimax") && !default.contains("glm"))
                } else {
                    true
                }
            })
            .map(|(i, _)| i)
            .collect();

        if matching.is_empty() {
            // Fallback: any available provider (ignoring model family)
            let available: Vec<usize> = (0..self.providers.len())
                .filter(|i| self.is_provider_available(*i))
                .collect();
            if available.is_empty() {
                // All circuits open — just round-robin (let them fail again to refresh cooldown)
                self.counter.fetch_add(1, Ordering::Relaxed) % self.providers.len()
            } else {
                let idx = self.counter.fetch_add(1, Ordering::Relaxed) % available.len();
                available[idx]
            }
        } else {
            let idx = self.counter.fetch_add(1, Ordering::Relaxed) % matching.len();
            matching[idx]
        }
    }

    /// Access the underlying providers list (for emergency fallback).
    pub fn providers(&self) -> &[Arc<dyn LlmProvider>] {
        &self.providers
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
                key, None, "claude-sonnet-4-6".to_string(),
            )));
        }

        // Gemini keys (check both GEMINI_API_KEY and GOOGLE_API_KEY)
        for key in Self::read_keys_multi(&["GEMINI_API_KEY", "GOOGLE_API_KEY"]) {
            providers.push(Arc::new(gemini::GeminiProvider::new(
                key, None, "gemini-2.5-flash".to_string(),
            )));
        }

        // Groq keys (fast inference)
        for key in Self::read_keys("GROQ_API_KEY") {
            providers.push(Arc::new(openai_compat::OpenAiCompatProvider::new(
                key, Some("https://api.groq.com/openai/v1".to_string()), "llama-3.3-70b-specdec".to_string(),
            )));
        }

        // DeepSeek keys
        for key in Self::read_keys("DEEPSEEK_API_KEY") {
            providers.push(Arc::new(openai_compat::OpenAiCompatProvider::new(
                key, Some("https://api.deepseek.com".to_string()), "deepseek-chat".to_string(),
            )));
        }

        // OpenRouter keys — cheap model chain: minimax → o4-mini → gemini-flash
        for key in Self::read_keys("OPENROUTER_API_KEY") {
            // MiniMax M2.5 — primary ($0.50/$1.50 per 1M, best cost-perf)
            providers.push(Arc::new(openai_compat::OpenAiCompatProvider::new(
                key.clone(), Some("https://openrouter.ai/api/v1".to_string()), "minimax/minimax-m2.5".to_string(),
            )));
            // Gemini 2.5 Flash — fallback ($0.15/$0.60 per 1M, cheapest)
            providers.push(Arc::new(openai_compat::OpenAiCompatProvider::new(
                key, Some("https://openrouter.ai/api/v1".to_string()), "google/gemini-2.5-flash-preview".to_string(),
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
        let req_is_deepseek = req_lower.contains("deepseek");
        let req_is_kimi = req_lower.contains("kimi") || req_lower.contains("moonshot");
        let req_is_qwen = req_lower.contains("qwen");
        let req_is_minimax = req_lower.contains("minimax");
        let req_is_glm = req_lower.contains("glm") || req_lower.contains("z-ai");

        let prov_is_claude = prov_default.contains("claude") || prov_default.contains("anthropic");
        let prov_is_gemini = prov_default.contains("gemini");
        let prov_is_groq = prov_default.contains("llama") || prov_default.contains("mixtral") || prov_default.contains("groq");
        let prov_is_deepseek = prov_default.contains("deepseek");
        let prov_is_kimi = prov_default.contains("kimi") || prov_default.contains("moonshot");
        let prov_is_qwen = prov_default.contains("qwen");
        let prov_is_minimax = prov_default.contains("minimax");
        let prov_is_glm = prov_default.contains("glm") || prov_default.contains("z-ai");
        let prov_is_openrouter = prov_default.contains("openrouter");
        let prov_is_gpt = !prov_is_claude && !prov_is_gemini && !prov_is_groq && !prov_is_deepseek && !prov_is_kimi && !prov_is_qwen && !prov_is_minimax && !prov_is_glm && !prov_is_openrouter;

        // Same family → use requested model as-is
        if (req_is_claude && prov_is_claude)
            || (req_is_gpt && prov_is_gpt)
            || (req_is_gemini && prov_is_gemini)
            || (req_is_groq && prov_is_groq)
            || (req_is_deepseek && prov_is_deepseek)
            || (req_is_kimi && prov_is_kimi)
            || (req_is_qwen && prov_is_qwen)
            || (req_is_minimax && prov_is_minimax)
            || (req_is_glm && prov_is_glm)
        {
            return requested_model.to_string();
        }

        // OpenRouter can handle any model — pass through the requested model
        // Also route minimax/glm models to OpenRouter (only available there)
        if prov_is_openrouter || prov_is_minimax || prov_is_glm || prov_is_kimi {
            return requested_model.to_string();
        }

        // Cross-family → map to the provider's best equivalent
        if prov_is_claude {
            "claude-sonnet-4-6".to_string()
        } else if prov_is_gemini {
            "gemini-2.5-flash".to_string()
        } else if prov_is_groq {
            "llama-3.3-70b-specdec".to_string()
        } else if prov_is_deepseek {
            "deepseek-chat".to_string()
        } else {
            // OpenAI-compatible
            "gpt-4o".to_string()
        }
    }

    /// Get list of available models for parallel racing.
    /// Returns (model_name, provider_index) pairs, one per provider family.
    /// Skips providers whose circuit breaker is open.
    pub fn available_parallel_models(&self) -> Vec<(String, usize)> {
        let mut models = Vec::new();
        let mut seen_families = std::collections::HashSet::new();
        for (i, p) in self.providers.iter().enumerate() {
            if !self.is_provider_available(i) { continue; }
            let default = p.default_model().to_lowercase();
            let family = if default.contains("claude") { "claude" }
                else if default.contains("gemini") { "gemini" }
                else if default.contains("llama") || default.contains("groq") { "groq" }
                else if default.contains("kimi") || default.contains("moonshot") { "kimi" }
                else if default.contains("qwen") { "qwen" }
                else if default.contains("minimax") { "minimax" }
                else if default.contains("glm") || default.contains("z-ai") { "glm" }
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

    /// Explore mode: run all providers in parallel and return ALL results (not just fastest).
    /// Results are sent via an mpsc channel as they arrive.
    /// Includes hierarchical re-query support: initial results can be escalated.
    pub async fn chat_explore(
        &self,
        messages: &[Message],
        tools: Option<&[serde_json::Value]>,
        max_tokens: u32,
        temperature: f64,
    ) -> Vec<ExploreResult> {
        let parallel_models = self.available_parallel_models();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<ExploreResult>(parallel_models.len() + 1);
        let msgs = messages.to_vec();
        let tools_owned: Option<Vec<serde_json::Value>> = tools.map(|t| t.to_vec());

        // Launch all remote providers in parallel
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
                        let elapsed = start.elapsed().as_millis() as u64;
                        let _ = tx.send(ExploreResult {
                            model: model.clone(),
                            response: resp.content.unwrap_or_default(),
                            response_time_ms: elapsed,
                            input_tokens: resp.usage.prompt_tokens,
                            output_tokens: resp.usage.completion_tokens,
                            is_fallback: false,
                        }).await;
                    }
                    Err(e) => {
                        tracing::warn!("Explore: {} failed: {}", model, e);
                    }
                }
            });
        }

        // Also run local fallback if available
        #[cfg(feature = "local-fallback")]
        {
            if let Some(local_provider) = local::LocalProvider::from_env() {
                let msgs = msgs.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    let start = std::time::Instant::now();
                    match local_provider.chat(&msgs, None, "local-qwen3-0.6b", max_tokens.min(512), temperature).await {
                        Ok(resp) => {
                            let elapsed = start.elapsed().as_millis() as u64;
                            let _ = tx.send(ExploreResult {
                                model: "local-qwen3-0.6b".to_string(),
                                response: resp.content.unwrap_or_default(),
                                response_time_ms: elapsed,
                                input_tokens: resp.usage.prompt_tokens,
                                output_tokens: resp.usage.completion_tokens,
                                is_fallback: true,
                            }).await;
                        }
                        Err(e) => {
                            tracing::warn!("Explore: local fallback failed: {}", e);
                        }
                    }
                });
            }
        }

        drop(tx); // close sender so rx will end when all tasks finish

        // Collect all results
        let mut results = Vec::new();
        while let Some(result) = rx.recv().await {
            results.push(result);
        }
        results
    }

    /// Race mode: run all providers in parallel and return ALL results ranked by completion order.
    /// Each result includes a 1-based rank (1 = fastest / winner).
    /// Timeout: 10 seconds per model; timed-out models are excluded.
    pub async fn chat_race(
        &self,
        messages: &[Message],
        tools: Option<&[serde_json::Value]>,
        max_tokens: u32,
        temperature: f64,
    ) -> Vec<RaceResult> {
        let parallel_models = self.available_parallel_models();
        let rank_counter = Arc::new(AtomicUsize::new(1));
        let (tx, mut rx) = tokio::sync::mpsc::channel::<RaceResult>(parallel_models.len() + 1);
        let msgs = messages.to_vec();
        let tools_owned: Option<Vec<serde_json::Value>> = tools.map(|t| t.to_vec());

        for (model_name, idx) in &parallel_models {
            let provider = self.providers[*idx].clone();
            let model = model_name.clone();
            let msgs = msgs.clone();
            let tools = tools_owned.clone();
            let tx = tx.clone();
            let rank_counter = rank_counter.clone();
            tokio::spawn(async move {
                let start = std::time::Instant::now();
                let tools_ref = tools.as_deref();
                match tokio::time::timeout(
                    std::time::Duration::from_secs(600),
                    provider.chat(&msgs, tools_ref, &model, max_tokens, temperature),
                ).await {
                    Ok(Ok(resp)) => {
                        let elapsed = start.elapsed().as_millis() as u64;
                        let rank = rank_counter.fetch_add(1, Ordering::SeqCst);
                        tracing::info!("Race: {} finished rank={} in {}ms", model, rank, elapsed);
                        let _ = tx.send(RaceResult {
                            model: model.clone(),
                            response: resp.content.unwrap_or_default(),
                            response_time_ms: elapsed,
                            input_tokens: resp.usage.prompt_tokens,
                            output_tokens: resp.usage.completion_tokens,
                            rank,
                            is_fallback: false,
                        }).await;
                    }
                    Ok(Err(e)) => {
                        tracing::warn!("Race: {} failed: {}", model, e);
                    }
                    Err(_) => {
                        tracing::warn!("Race: {} timed out (10s)", model);
                    }
                }
            });
        }

        // Also run local fallback if available
        #[cfg(feature = "local-fallback")]
        {
            if let Some(local_provider) = local::LocalProvider::from_env() {
                let msgs = msgs.clone();
                let tx = tx.clone();
                let rank_counter = rank_counter.clone();
                tokio::spawn(async move {
                    let start = std::time::Instant::now();
                    match local_provider.chat(&msgs, None, "local-qwen3-0.6b", max_tokens.min(512), temperature).await {
                        Ok(resp) => {
                            let elapsed = start.elapsed().as_millis() as u64;
                            let rank = rank_counter.fetch_add(1, Ordering::SeqCst);
                            let _ = tx.send(RaceResult {
                                model: "local-qwen3-0.6b".to_string(),
                                response: resp.content.unwrap_or_default(),
                                response_time_ms: elapsed,
                                input_tokens: resp.usage.prompt_tokens,
                                output_tokens: resp.usage.completion_tokens,
                                rank,
                                is_fallback: true,
                            }).await;
                        }
                        Err(e) => {
                            tracing::warn!("Race: local fallback failed: {}", e);
                        }
                    }
                });
            }
        }

        drop(tx);

        let mut results = Vec::new();
        while let Some(result) = rx.recv().await {
            results.push(result);
        }
        // Sort by rank (completion order)
        results.sort_by_key(|r| r.rank);
        results
    }

    /// Race mode with streaming: run all providers in parallel and stream results as they arrive.
    /// Returns a Receiver that yields RaceResult in completion order (fastest first).
    /// Use this for real-time SSE streaming in explore mode.
    pub async fn chat_race_stream(
        &self,
        messages: &[Message],
        tools: Option<&[serde_json::Value]>,
        max_tokens: u32,
        temperature: f64,
    ) -> tokio::sync::mpsc::Receiver<RaceResult> {
        let parallel_models = self.available_parallel_models();
        let rank_counter = Arc::new(AtomicUsize::new(1));
        let (tx, rx) = tokio::sync::mpsc::channel::<RaceResult>(parallel_models.len() + 1);
        let msgs = messages.to_vec();
        let tools_owned: Option<Vec<serde_json::Value>> = tools.map(|t| t.to_vec());

        for (model_name, idx) in &parallel_models {
            let provider = self.providers[*idx].clone();
            let model = model_name.clone();
            let msgs = msgs.clone();
            let tools = tools_owned.clone();
            let tx = tx.clone();
            let rank_counter = rank_counter.clone();
            tokio::spawn(async move {
                let start = std::time::Instant::now();
                let tools_ref = tools.as_deref();
                match tokio::time::timeout(
                    std::time::Duration::from_secs(600),
                    provider.chat(&msgs, tools_ref, &model, max_tokens, temperature),
                ).await {
                    Ok(Ok(resp)) => {
                        let elapsed = start.elapsed().as_millis() as u64;
                        let rank = rank_counter.fetch_add(1, Ordering::SeqCst);
                        tracing::info!("Race stream: {} finished rank={} in {}ms", model, rank, elapsed);
                        let _ = tx.send(RaceResult {
                            model: model.clone(),
                            response: resp.content.unwrap_or_default(),
                            response_time_ms: elapsed,
                            input_tokens: resp.usage.prompt_tokens,
                            output_tokens: resp.usage.completion_tokens,
                            rank,
                            is_fallback: false,
                        }).await;
                    }
                    Ok(Err(e)) => {
                        tracing::warn!("Race stream: {} failed: {}", model, e);
                    }
                    Err(_) => {
                        tracing::warn!("Race stream: {} timed out (10s)", model);
                    }
                }
            });
        }

        // Also run local fallback if available
        #[cfg(feature = "local-fallback")]
        {
            if let Some(local_provider) = local::LocalProvider::from_env() {
                let msgs = msgs.clone();
                let tx = tx.clone();
                let rank_counter = rank_counter.clone();
                tokio::spawn(async move {
                    let start = std::time::Instant::now();
                    match local_provider.chat(&msgs, None, "local-qwen3-0.6b", max_tokens.min(512), temperature).await {
                        Ok(resp) => {
                            let elapsed = start.elapsed().as_millis() as u64;
                            let rank = rank_counter.fetch_add(1, Ordering::SeqCst);
                            let _ = tx.send(RaceResult {
                                model: "local-qwen3-0.6b".to_string(),
                                response: resp.content.unwrap_or_default(),
                                response_time_ms: elapsed,
                                input_tokens: resp.usage.prompt_tokens,
                                output_tokens: resp.usage.completion_tokens,
                                rank,
                                is_fallback: true,
                            }).await;
                        }
                        Err(e) => {
                            tracing::warn!("Race stream: local fallback failed: {}", e);
                        }
                    }
                });
            }
        }

        // Drop tx so that rx closes when all spawned tasks complete
        drop(tx);

        rx
    }

    /// Get a specific provider for a single-model tier request.
    /// Returns (provider, model_name) or None if not found.
    pub fn get_tier_model(&self, tier: &str) -> Option<(Arc<dyn LlmProvider>, String)> {
        // Each tier has a fallback chain: primary → secondary → tertiary
        let candidates: &[&str] = match tier {
            "economy"  => &["gemini-2.5-flash", "deepseek-chat", "llama-3.3-70b-specdec"],
            "normal"   => &["minimax/minimax-m2.5", "google/gemini-2.5-flash-preview"],
            "powerful" => &["claude-sonnet-4-6", "gpt-4o", "gemini-2.5-pro"],
            _ => return None,
        };
        for candidate in candidates {
            let target_lower = candidate.to_lowercase();
            // Exact match first
            for (i, p) in self.providers.iter().enumerate() {
                if p.default_model().to_lowercase() == target_lower {
                    return Some((self.providers[i].clone(), candidate.to_string()));
                }
            }
            // Partial match
            for (i, p) in self.providers.iter().enumerate() {
                let d = p.default_model().to_lowercase();
                if target_lower.contains(&d) || d.contains(&target_lower) {
                    return Some((self.providers[i].clone(), candidate.to_string()));
                }
            }
        }
        None
    }
}

/// Result from a single model in explore mode.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExploreResult {
    pub model: String,
    pub response: String,
    pub response_time_ms: u64,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub is_fallback: bool,
}

/// Result from a single model in race mode (ranked by completion order).
#[derive(Debug, Clone, serde::Serialize)]
pub struct RaceResult {
    pub model: String,
    pub response: String,
    pub response_time_ms: u64,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub rank: usize,
    pub is_fallback: bool,
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
        if total == 0 {
            return Err(ProviderError::Other("No providers configured".to_string()));
        }

        // Fast failover: primary gets a head start (3s), then all providers race in parallel
        let primary_head_start = std::time::Duration::from_secs(3);
        let parallel_timeout = std::time::Duration::from_secs(7);

        // Phase 1: Try primary provider with short timeout
        let primary_idx = self.select_provider_idx(model);
        let primary = &*self.providers[primary_idx];
        let primary_result = tokio::time::timeout(
            primary_head_start,
            primary.chat(messages, tools, model, max_tokens, temperature),
        ).await;

        match primary_result {
            Ok(Ok(resp)) => {
                self.record_success(primary_idx);
                return Ok(resp);
            }
            Ok(Err(e)) => {
                self.record_failure_if_server_error(primary_idx, &e);
                tracing::warn!("Primary provider failed for model {}: {}, trying parallel fallback", model, e);
            }
            Err(_) => {
                tracing::warn!("Primary provider slow for model {} (>{}s), racing all fallbacks", model, primary_head_start.as_secs());
            }
        }

        // Phase 2: Race ALL remaining available providers in parallel, return first success
        if total > 1 {
            let start = self.counter.load(Ordering::Relaxed);
            let msgs = messages.to_vec();
            let tools_owned: Option<Vec<serde_json::Value>> = tools.map(|t| t.to_vec());

            let (tx, mut rx) = tokio::sync::mpsc::channel::<(CompletionResponse, usize)>(total);
            let (fail_tx, mut fail_rx) = tokio::sync::mpsc::channel::<usize>(total);

            let mut spawned = 0;
            for i in 1..total {
                let idx = (start + i) % total;
                if !self.is_provider_available(idx) { continue; }
                let provider = self.providers[idx].clone();
                let converted_model = Self::convert_model_for_provider(provider.as_ref(), model);
                let msgs = msgs.clone();
                let tools = tools_owned.clone();
                let tx = tx.clone();
                let fail_tx = fail_tx.clone();

                tokio::spawn(async move {
                    let tools_ref = tools.as_deref();
                    match tokio::time::timeout(
                        parallel_timeout,
                        provider.chat(&msgs, tools_ref, &converted_model, max_tokens, temperature),
                    ).await {
                        Ok(Ok(resp)) => {
                            tracing::info!("Parallel fallback succeeded with model {}", converted_model);
                            let _ = tx.send((resp, idx)).await;
                        }
                        Ok(Err(e)) => {
                            tracing::warn!("Parallel fallback {} failed: {}", converted_model, e);
                            // Only count server errors for circuit breaker (not 4xx client errors)
                            let is_server_error = !matches!(&e, crate::error::ProviderError::Api { status, .. } if *status < 500);
                            if is_server_error {
                                let _ = fail_tx.send(idx).await;
                            }
                        }
                        Err(_) => {
                            tracing::warn!("Parallel fallback {} timed out ({}s)", converted_model, parallel_timeout.as_secs());
                        }
                    }
                });
                spawned += 1;
            }
            drop(tx);
            drop(fail_tx);

            if spawned > 0 {
                // Collect failures and first success
                while let Ok(idx) = fail_rx.try_recv() {
                    self.record_failure(idx);
                }
                if let Some((resp, success_idx)) = rx.recv().await {
                    self.record_success(success_idx);
                    // Drain remaining failures
                    while let Ok(idx) = fail_rx.try_recv() {
                        self.record_failure(idx);
                    }
                    return Ok(resp);
                }
                // All failed — drain remaining failures
                while let Ok(idx) = fail_rx.try_recv() {
                    self.record_failure(idx);
                }
            }
        }

        // Phase 3: Local fallback (if feature enabled)
        #[cfg(feature = "local-fallback")]
        {
            tracing::warn!("All remote providers failed — trying local fallback");
            if let Some(local_provider) = local::LocalProvider::from_env() {
                match local_provider.chat(messages, tools, "local-qwen3-0.6b", max_tokens.min(512), temperature).await {
                    Ok(resp) => {
                        tracing::info!("Local fallback succeeded");
                        return Ok(resp);
                    }
                    Err(e) => tracing::error!("Local fallback also failed: {}", e),
                }
            }
        }

        Err(ProviderError::Other("All providers failed".to_string()))
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
        let total = self.providers.len();
        if total == 0 {
            return Err(ProviderError::Other("No providers configured".to_string()));
        }

        // Sequential failover for streaming (can't race — each provider writes to the same chunk_tx)
        let start = self.counter.load(Ordering::Relaxed);
        let mut last_err = String::new();

        for i in 0..total {
            let idx = (start + i) % total;
            if !self.is_provider_available(idx) {
                tracing::debug!("Stream: skipping provider #{} (circuit open)", idx);
                continue;
            }
            let provider = &*self.providers[idx];
            let converted_model = Self::convert_model_for_provider(provider, model);

            match tokio::time::timeout(
                std::time::Duration::from_secs(600),
                provider.chat_stream(messages, tools, &converted_model, max_tokens, temperature, extra, chunk_tx.clone()),
            ).await {
                Ok(Ok(resp)) => {
                    self.record_success(idx);
                    if i > 0 {
                        tracing::info!("Stream failover succeeded with provider #{} model {}", idx, converted_model);
                    }
                    return Ok(resp);
                }
                Ok(Err(e)) => {
                    self.record_failure_if_server_error(idx, &e);
                    tracing::warn!("Stream provider #{} ({}) failed: {}, trying next", idx, converted_model, e);
                    last_err = format!("{}", e);
                }
                Err(_) => {
                    tracing::warn!("Stream provider #{} ({}) timed out (10s), trying next", idx, converted_model);
                    last_err = "timeout".to_string();
                }
            }
        }

        Err(ProviderError::Other(format!("All {} stream providers failed: {}", total, last_err)))
    }

    fn default_model(&self) -> &str {
        self.providers.first().map(|p| p.default_model()).unwrap_or("gpt-4o")
    }
}
