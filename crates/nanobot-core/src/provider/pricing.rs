//! Model pricing table — single source of truth for cost calculation, credit rates, and display.

/// Pricing information for a single LLM model.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ModelPricing {
    pub model: &'static str,
    pub provider: &'static str,
    pub input_per_1m: f64,
    pub output_per_1m: f64,
    pub context_window: u32,
    /// Credits consumed per 1K input tokens.
    pub credits_in_1k: u32,
    /// Credits consumed per 1K output tokens.
    pub credits_out_1k: u32,
}

/// Static pricing table for all supported models.
/// This is the SINGLE SOURCE OF TRUTH for all pricing/cost/credit information.
/// Changes here automatically propagate to:
///   - /api/v1/models (frontend model picker)
///   - /api/v1/pricing (pricing page)
///   - /v1/models (OpenAI-compatible)
///   - credit_rate() in auth.rs
///   - Frontend tier labels, cost displays, calculators
pub const PRICING_TABLE: &[ModelPricing] = &[
    // ── OpenAI ──
    ModelPricing { model: "gpt-4o",          provider: "openai",    input_per_1m: 2.50,  output_per_1m: 10.00, context_window: 128_000,   credits_in_1k: 5,  credits_out_1k: 15 },
    ModelPricing { model: "gpt-4o-mini",     provider: "openai",    input_per_1m: 0.15,  output_per_1m: 0.60,  context_window: 128_000,   credits_in_1k: 1,  credits_out_1k: 3 },
    ModelPricing { model: "gpt-4.1",         provider: "openai",    input_per_1m: 2.00,  output_per_1m: 8.00,  context_window: 1_048_576, credits_in_1k: 5,  credits_out_1k: 15 },
    ModelPricing { model: "gpt-4.1-mini",    provider: "openai",    input_per_1m: 0.40,  output_per_1m: 1.60,  context_window: 1_048_576, credits_in_1k: 1,  credits_out_1k: 4 },
    ModelPricing { model: "gpt-4.1-nano",    provider: "openai",    input_per_1m: 0.10,  output_per_1m: 0.40,  context_window: 1_048_576, credits_in_1k: 1,  credits_out_1k: 3 },
    ModelPricing { model: "o3-mini",         provider: "openai",    input_per_1m: 1.10,  output_per_1m: 4.40,  context_window: 200_000,   credits_in_1k: 2,  credits_out_1k: 8 },
    ModelPricing { model: "o4-mini",         provider: "openai",    input_per_1m: 1.10,  output_per_1m: 4.40,  context_window: 200_000,   credits_in_1k: 2,  credits_out_1k: 8 },
    // ── Anthropic ──
    ModelPricing { model: "claude-sonnet-4-5-20250929", provider: "anthropic", input_per_1m: 3.00,  output_per_1m: 15.00, context_window: 200_000, credits_in_1k: 6,  credits_out_1k: 18 },
    ModelPricing { model: "claude-sonnet-4-6",          provider: "anthropic", input_per_1m: 3.00,  output_per_1m: 15.00, context_window: 200_000, credits_in_1k: 6,  credits_out_1k: 18 },
    ModelPricing { model: "claude-haiku-4-5-20251001",  provider: "anthropic", input_per_1m: 1.00,  output_per_1m: 5.00,  context_window: 200_000, credits_in_1k: 2,  credits_out_1k: 8 },
    ModelPricing { model: "claude-opus-4-5",   provider: "anthropic", input_per_1m: 5.00,  output_per_1m: 25.00, context_window: 200_000, credits_in_1k: 10, credits_out_1k: 38 },
    ModelPricing { model: "claude-opus-4-6",   provider: "anthropic", input_per_1m: 5.00,  output_per_1m: 25.00, context_window: 200_000, credits_in_1k: 10, credits_out_1k: 38 },
    // ── Google ──
    ModelPricing { model: "gemini-2.5-flash",      provider: "google", input_per_1m: 0.15, output_per_1m: 0.60, context_window: 1_048_576, credits_in_1k: 1, credits_out_1k: 3 },
    ModelPricing { model: "gemini-2.5-flash-lite",  provider: "google", input_per_1m: 0.10, output_per_1m: 0.40, context_window: 1_048_576, credits_in_1k: 1, credits_out_1k: 3 },
    ModelPricing { model: "gemini-2.5-pro",         provider: "google", input_per_1m: 1.25, output_per_1m: 10.00, context_window: 1_048_576, credits_in_1k: 3, credits_out_1k: 15 },
    ModelPricing { model: "gemini-2.0-flash",       provider: "google", input_per_1m: 0.10, output_per_1m: 0.40, context_window: 1_048_576, credits_in_1k: 1, credits_out_1k: 3 },
    ModelPricing { model: "google/gemini-3-flash-preview", provider: "google", input_per_1m: 0.15, output_per_1m: 0.60, context_window: 1_048_576, credits_in_1k: 1, credits_out_1k: 3 },
    // ── Groq (ultra-fast inference) ──
    ModelPricing { model: "llama-3.3-70b-specdec",  provider: "groq",  input_per_1m: 0.59, output_per_1m: 0.79, context_window: 128_000, credits_in_1k: 1, credits_out_1k: 2 },
    ModelPricing { model: "llama-4-scout-17b-16e",  provider: "groq",  input_per_1m: 0.11, output_per_1m: 0.34, context_window: 131_072, credits_in_1k: 1, credits_out_1k: 1 },
    ModelPricing { model: "llama-4-maverick-17b-128e", provider: "groq", input_per_1m: 0.20, output_per_1m: 0.60, context_window: 131_072, credits_in_1k: 1, credits_out_1k: 2 },
    ModelPricing { model: "gemma2-9b-it",           provider: "groq",  input_per_1m: 0.20, output_per_1m: 0.20, context_window: 8_192,   credits_in_1k: 1, credits_out_1k: 1 },
    ModelPricing { model: "mistral-saba-24b",       provider: "groq",  input_per_1m: 0.79, output_per_1m: 0.79, context_window: 32_768,  credits_in_1k: 1, credits_out_1k: 2 },
    ModelPricing { model: "qwen-qwq-32b",           provider: "groq",  input_per_1m: 0.29, output_per_1m: 0.39, context_window: 131_072, credits_in_1k: 1, credits_out_1k: 1 },
    // ── Kimi / Moonshot ──
    ModelPricing { model: "kimi-k2-0711",     provider: "moonshot",  input_per_1m: 0.60, output_per_1m: 2.40, context_window: 131_072, credits_in_1k: 3, credits_out_1k: 9 },
    ModelPricing { model: "moonshotai/kimi-k2.5", provider: "openrouter", input_per_1m: 1.00, output_per_1m: 3.00, context_window: 131_072, credits_in_1k: 3, credits_out_1k: 9 },
    // ── DeepSeek ──
    ModelPricing { model: "deepseek-chat",      provider: "deepseek",    input_per_1m: 0.28, output_per_1m: 0.42, context_window: 128_000, credits_in_1k: 1, credits_out_1k: 1 },
    ModelPricing { model: "deepseek-reasoner",  provider: "deepseek",    input_per_1m: 0.55, output_per_1m: 2.19, context_window: 128_000, credits_in_1k: 1, credits_out_1k: 4 },
    // ── OpenRouter (日本語対応モデル多数) ──
    ModelPricing { model: "openrouter/auto",      provider: "openrouter", input_per_1m: 1.00, output_per_1m: 3.00, context_window: 131_072, credits_in_1k: 3, credits_out_1k: 9 },
    ModelPricing { model: "minimax/minimax-m2.5", provider: "openrouter", input_per_1m: 0.50, output_per_1m: 1.50, context_window: 131_072, credits_in_1k: 2, credits_out_1k: 6 },
    ModelPricing { model: "z-ai/glm-5",          provider: "openrouter", input_per_1m: 0.60, output_per_1m: 2.40, context_window: 204_800, credits_in_1k: 3, credits_out_1k: 9 },
    // Meta Llama (via OpenRouter — 日本語対応)
    ModelPricing { model: "meta-llama/llama-4-scout",   provider: "openrouter", input_per_1m: 0.15, output_per_1m: 0.60, context_window: 512_000, credits_in_1k: 1, credits_out_1k: 2 },
    ModelPricing { model: "meta-llama/llama-4-maverick", provider: "openrouter", input_per_1m: 0.25, output_per_1m: 1.00, context_window: 1_048_576, credits_in_1k: 1, credits_out_1k: 3 },
    // Qwen (via OpenRouter — 日本語・中国語に強い)
    ModelPricing { model: "qwen/qwen3-235b-a22b",       provider: "openrouter", input_per_1m: 0.20, output_per_1m: 0.60, context_window: 131_072, credits_in_1k: 1, credits_out_1k: 2 },
    ModelPricing { model: "qwen/qwen3-30b-a3b",         provider: "openrouter", input_per_1m: 0.07, output_per_1m: 0.15, context_window: 131_072, credits_in_1k: 1, credits_out_1k: 1 },
    ModelPricing { model: "qwen/qwen3-32b",             provider: "openrouter", input_per_1m: 0.10, output_per_1m: 0.30, context_window: 131_072, credits_in_1k: 1, credits_out_1k: 1 },
    ModelPricing { model: "qwen/qwq-32b",               provider: "openrouter", input_per_1m: 0.10, output_per_1m: 0.30, context_window: 131_072, credits_in_1k: 1, credits_out_1k: 1 },
    ModelPricing { model: "qwen/qwen3-coder",            provider: "openrouter", input_per_1m: 0.20, output_per_1m: 0.60, context_window: 262_144, credits_in_1k: 1, credits_out_1k: 2 },
    // Qwen 3.5 (via OpenRouter — マルチモーダル・ネイティブエージェント)
    ModelPricing { model: "qwen/qwen3.5-9b",              provider: "openrouter", input_per_1m: 0.05, output_per_1m: 0.15, context_window: 131_072, credits_in_1k: 1, credits_out_1k: 1 },
    ModelPricing { model: "qwen/qwen3.5-27b",             provider: "openrouter", input_per_1m: 0.20, output_per_1m: 1.56, context_window: 131_072, credits_in_1k: 1, credits_out_1k: 3 },
    ModelPricing { model: "qwen/qwen3.5-35b-a3b",         provider: "openrouter", input_per_1m: 0.16, output_per_1m: 1.30, context_window: 131_072, credits_in_1k: 1, credits_out_1k: 3 },
    ModelPricing { model: "qwen/qwen3.5-122b-a10b",       provider: "openrouter", input_per_1m: 0.26, output_per_1m: 2.08, context_window: 131_072, credits_in_1k: 1, credits_out_1k: 4 },
    ModelPricing { model: "qwen/qwen3.5-397b-a17b",       provider: "openrouter", input_per_1m: 0.39, output_per_1m: 2.34, context_window: 131_072, credits_in_1k: 1, credits_out_1k: 4 },
    ModelPricing { model: "qwen/qwen3.5-plus-02-15",      provider: "openrouter", input_per_1m: 0.26, output_per_1m: 1.56, context_window: 1_048_576, credits_in_1k: 1, credits_out_1k: 3 },
    ModelPricing { model: "qwen/qwen3.5-flash-02-23",     provider: "openrouter", input_per_1m: 0.07, output_per_1m: 0.26, context_window: 131_072, credits_in_1k: 1, credits_out_1k: 1 },
    // Mistral (via OpenRouter — 日本語対応)
    ModelPricing { model: "mistralai/mistral-large",     provider: "openrouter", input_per_1m: 2.00, output_per_1m: 6.00, context_window: 131_072, credits_in_1k: 4, credits_out_1k: 12 },
    ModelPricing { model: "mistralai/mistral-small-3.2", provider: "openrouter", input_per_1m: 0.10, output_per_1m: 0.30, context_window: 131_072, credits_in_1k: 1, credits_out_1k: 1 },
    ModelPricing { model: "mistralai/codestral",         provider: "openrouter", input_per_1m: 0.30, output_per_1m: 0.90, context_window: 262_144, credits_in_1k: 1, credits_out_1k: 3 },
    // Cohere (via OpenRouter — 日本語特化学習済み)
    ModelPricing { model: "cohere/command-r-plus",       provider: "openrouter", input_per_1m: 2.50, output_per_1m: 10.00, context_window: 128_000, credits_in_1k: 5, credits_out_1k: 15 },
    ModelPricing { model: "cohere/command-r",            provider: "openrouter", input_per_1m: 0.15, output_per_1m: 0.60,  context_window: 128_000, credits_in_1k: 1, credits_out_1k: 2 },
    ModelPricing { model: "cohere/command-a",            provider: "openrouter", input_per_1m: 2.50, output_per_1m: 10.00, context_window: 256_000, credits_in_1k: 5, credits_out_1k: 15 },
    // NVIDIA (via OpenRouter)
    ModelPricing { model: "nvidia/llama-3.1-nemotron-70b", provider: "openrouter", input_per_1m: 0.35, output_per_1m: 0.40, context_window: 131_072, credits_in_1k: 1, credits_out_1k: 1 },
    // xAI (via OpenRouter)
    ModelPricing { model: "x-ai/grok-3-mini",    provider: "openrouter", input_per_1m: 0.30, output_per_1m: 0.50, context_window: 131_072, credits_in_1k: 1, credits_out_1k: 2 },
    ModelPricing { model: "x-ai/grok-3",         provider: "openrouter", input_per_1m: 3.00, output_per_1m: 15.00, context_window: 131_072, credits_in_1k: 6, credits_out_1k: 18 },
    // Microsoft Phi (via OpenRouter — 軽量日本語)
    ModelPricing { model: "microsoft/phi-4",      provider: "openrouter", input_per_1m: 0.07, output_per_1m: 0.07, context_window: 16_384, credits_in_1k: 1, credits_out_1k: 1 },
    // Google Gemma (via OpenRouter — 日本語対応)
    ModelPricing { model: "google/gemma-3-27b-it", provider: "openrouter", input_per_1m: 0.10, output_per_1m: 0.20, context_window: 131_072, credits_in_1k: 1, credits_out_1k: 1 },
    // ── DeepInfra ──
    ModelPricing { model: "nvidia/NVIDIA-Nemotron-Nano-9B-v2-Japanese", provider: "deepinfra", input_per_1m: 0.04, output_per_1m: 0.16, context_window: 128_000, credits_in_1k: 1, credits_out_1k: 1 },
    ModelPricing { model: "nvidia/Llama-3.3-Nemotron-Super-49B-v1.5", provider: "deepinfra", input_per_1m: 0.10, output_per_1m: 0.40, context_window: 131_072, credits_in_1k: 1, credits_out_1k: 2 },
    ModelPricing { model: "nvidia/NVIDIA-Nemotron-3-Super-120B-A12B", provider: "deepinfra", input_per_1m: 0.20, output_per_1m: 0.65, context_window: 1_048_576, credits_in_1k: 1, credits_out_1k: 3 },
    // ── RunPod GPU Pods (self-hosted vLLM, 無料/格安) ──
    ModelPricing { model: "nemotron-9b-jp",   provider: "runpod", input_per_1m: 0.00, output_per_1m: 0.00, context_window: 8_192,   credits_in_1k: 0, credits_out_1k: 0 },
    ModelPricing { model: "qwen3-32b",        provider: "runpod", input_per_1m: 0.20, output_per_1m: 0.60, context_window: 16_384,  credits_in_1k: 1, credits_out_1k: 2 },
    ModelPricing { model: "kimi-k2.5",        provider: "runpod", input_per_1m: 0.15, output_per_1m: 0.60, context_window: 131_072, credits_in_1k: 1, credits_out_1k: 2 },
    ModelPricing { model: "qwen3-coder-480b", provider: "runpod", input_per_1m: 0.20, output_per_1m: 0.60, context_window: 32_768,  credits_in_1k: 1, credits_out_1k: 2 },
    ModelPricing { model: "futa-2b",          provider: "runpod", input_per_1m: 0.02, output_per_1m: 0.08, context_window: 4_096,   credits_in_1k: 1, credits_out_1k: 1 },
];

/// Media generation pricing (per unit).
#[derive(Debug, Clone, serde::Serialize)]
pub struct MediaPricing {
    pub service: &'static str,
    pub tier: &'static str,
    pub price_usd: f64,
    pub unit: &'static str,
}

pub const MEDIA_PRICING: &[MediaPricing] = &[
    // Image generation (gpt-image-1)
    MediaPricing { service: "image_generate", tier: "low",    price_usd: 0.011, unit: "image" },
    MediaPricing { service: "image_generate", tier: "medium", price_usd: 0.042, unit: "image" },
    MediaPricing { service: "image_generate", tier: "high",   price_usd: 0.167, unit: "image" },
    // Music generation (Suno)
    MediaPricing { service: "music_generate", tier: "standard", price_usd: 0.05, unit: "song" },
    // Video generation (Kling)
    MediaPricing { service: "video_generate", tier: "standard_5s", price_usd: 0.10, unit: "video" },
    MediaPricing { service: "video_generate", tier: "pro_5s",      price_usd: 0.35, unit: "video" },
    MediaPricing { service: "video_generate", tier: "standard_10s", price_usd: 0.20, unit: "video" },
    MediaPricing { service: "video_generate", tier: "pro_10s",      price_usd: 0.70, unit: "video" },
];

/// Look up pricing for a model. Tries exact match first, then best fuzzy match.
pub fn lookup_model(model: &str) -> Option<&'static ModelPricing> {
    let lower = model.to_lowercase();
    // Exact match first (case-insensitive)
    if let Some(p) = PRICING_TABLE.iter().find(|p| p.model.to_lowercase() == lower) {
        return Some(p);
    }
    // Best fuzzy match: prefer longest matching model name (case-insensitive)
    PRICING_TABLE.iter()
        .filter(|p| {
            let p_lower = p.model.to_lowercase();
            lower.contains(&*p_lower) || p_lower.contains(&*lower)
        })
        .max_by_key(|p| p.model.len())
}

/// Calculate cost in USD for a given number of input/output tokens.
pub fn calculate_cost(model: &str, input_tokens: u32, output_tokens: u32) -> f64 {
    if let Some(pricing) = lookup_model(model) {
        let input_cost = (input_tokens as f64 / 1_000_000.0) * pricing.input_per_1m;
        let output_cost = (output_tokens as f64 / 1_000_000.0) * pricing.output_per_1m;
        input_cost + output_cost
    } else {
        // Fallback: assume mid-range pricing
        let input_cost = (input_tokens as f64 / 1_000_000.0) * 2.0;
        let output_cost = (output_tokens as f64 / 1_000_000.0) * 10.0;
        input_cost + output_cost
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lookup_model() {
        let p = lookup_model("gpt-4o").unwrap();
        assert_eq!(p.provider, "openai");
        assert!((p.input_per_1m - 2.50).abs() < f64::EPSILON);
        assert_eq!(p.credits_in_1k, 5);
        assert_eq!(p.credits_out_1k, 15);

        let p2 = lookup_model("claude-sonnet-4-5-20250929").unwrap();
        assert_eq!(p2.provider, "anthropic");

        let p3 = lookup_model("claude-sonnet-4-6").unwrap();
        assert_eq!(p3.provider, "anthropic");
        assert_eq!(p3.credits_in_1k, 6);

        let p4 = lookup_model("deepseek-reasoner").unwrap();
        assert_eq!(p4.provider, "deepseek");

        let p5 = lookup_model("minimax/minimax-m2.5").unwrap();
        assert_eq!(p5.provider, "openrouter");
        assert_eq!(p5.credits_in_1k, 2);
        assert_eq!(p5.credits_out_1k, 6);

        let p6 = lookup_model("z-ai/glm-5").unwrap();
        assert_eq!(p6.provider, "openrouter");

        let p7 = lookup_model("google/gemini-3-flash-preview").unwrap();
        assert_eq!(p7.provider, "google");
    }

    #[test]
    fn test_calculate_cost() {
        let cost = calculate_cost("gpt-4o", 1000, 500);
        // input: 1000/1M * 2.50 = 0.0025, output: 500/1M * 10.00 = 0.005
        let expected = 0.0025 + 0.005;
        assert!((cost - expected).abs() < 1e-6);
    }

    #[test]
    fn test_unknown_model_fallback() {
        let cost = calculate_cost("unknown-model-xyz", 1000, 500);
        assert!(cost > 0.0);
    }
}
