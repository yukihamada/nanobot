//! Model pricing table — static reference data for cost calculation and display.

/// Pricing information for a single LLM model.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ModelPricing {
    pub model: &'static str,
    pub provider: &'static str,
    pub input_per_1m: f64,
    pub output_per_1m: f64,
    pub context_window: u32,
}

/// Static pricing table for all supported models.
pub const PRICING_TABLE: &[ModelPricing] = &[
    // OpenAI
    ModelPricing { model: "gpt-4o",          provider: "openai",    input_per_1m: 2.50,  output_per_1m: 10.00, context_window: 128_000 },
    ModelPricing { model: "gpt-4o-mini",     provider: "openai",    input_per_1m: 0.15,  output_per_1m: 0.60,  context_window: 128_000 },
    ModelPricing { model: "gpt-4.1",         provider: "openai",    input_per_1m: 2.00,  output_per_1m: 8.00,  context_window: 1_048_576 },
    ModelPricing { model: "gpt-4.1-mini",    provider: "openai",    input_per_1m: 0.40,  output_per_1m: 1.60,  context_window: 1_048_576 },
    ModelPricing { model: "gpt-4.1-nano",    provider: "openai",    input_per_1m: 0.10,  output_per_1m: 0.40,  context_window: 1_048_576 },
    ModelPricing { model: "o3-mini",         provider: "openai",    input_per_1m: 1.10,  output_per_1m: 4.40,  context_window: 200_000 },
    ModelPricing { model: "o4-mini",         provider: "openai",    input_per_1m: 1.10,  output_per_1m: 4.40,  context_window: 200_000 },
    // Anthropic
    ModelPricing { model: "claude-sonnet-4-5-20250929", provider: "anthropic", input_per_1m: 3.00,  output_per_1m: 15.00, context_window: 200_000 },
    ModelPricing { model: "claude-haiku-4-5-20251001",  provider: "anthropic", input_per_1m: 1.00,  output_per_1m: 5.00,  context_window: 200_000 },
    ModelPricing { model: "claude-opus-4-5",   provider: "anthropic", input_per_1m: 5.00,  output_per_1m: 25.00, context_window: 200_000 },
    ModelPricing { model: "claude-opus-4-6",   provider: "anthropic", input_per_1m: 5.00,  output_per_1m: 25.00, context_window: 200_000 },
    // Google
    ModelPricing { model: "gemini-2.5-flash",      provider: "google", input_per_1m: 0.15, output_per_1m: 0.60, context_window: 1_048_576 },
    ModelPricing { model: "gemini-2.5-flash-lite",  provider: "google", input_per_1m: 0.10, output_per_1m: 0.40, context_window: 1_048_576 },
    ModelPricing { model: "gemini-2.5-pro",         provider: "google", input_per_1m: 1.25, output_per_1m: 10.00, context_window: 1_048_576 },
    ModelPricing { model: "gemini-2.0-flash",       provider: "google", input_per_1m: 0.10, output_per_1m: 0.40, context_window: 1_048_576 },
    // Groq (fast inference)
    ModelPricing { model: "llama-3.3-70b-versatile", provider: "groq",  input_per_1m: 0.59, output_per_1m: 0.79, context_window: 128_000 },
    // Kimi / Moonshot
    ModelPricing { model: "kimi-k2-0711",     provider: "moonshot",  input_per_1m: 0.60, output_per_1m: 2.40, context_window: 131_072 },
    ModelPricing { model: "moonshotai/kimi-k2-instruct-0905", provider: "groq", input_per_1m: 1.00, output_per_1m: 3.00, context_window: 131_072 },
    // Qwen (via Groq)
    ModelPricing { model: "qwen/qwen3-32b",   provider: "groq",     input_per_1m: 0.50, output_per_1m: 1.50, context_window: 131_072 },
    // DeepSeek
    ModelPricing { model: "deepseek-chat",    provider: "deepseek",  input_per_1m: 0.28, output_per_1m: 0.42, context_window: 128_000 },
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

/// Look up pricing for a model (fuzzy match on prefix/contains).
pub fn lookup_model(model: &str) -> Option<&'static ModelPricing> {
    let lower = model.to_lowercase();
    PRICING_TABLE.iter().find(|p| {
        lower.contains(p.model) || p.model.contains(&*lower)
    })
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

        let p2 = lookup_model("claude-sonnet-4-5-20250929").unwrap();
        assert_eq!(p2.provider, "anthropic");
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
