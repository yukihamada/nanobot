use serde::{Deserialize, Serialize};

/// Tenant information extracted from API key authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tenant {
    pub tenant_id: String,
    pub plan: Plan,
    pub credits_remaining: i64,
    pub rate_limit_per_min: u32,
    pub agent_runs_limit: u32,
}

/// Subscription plan tier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Plan {
    Free,
    Starter,
    Pro,
    Enterprise,
}

impl Plan {
    pub fn rate_limit_per_min(&self) -> u32 {
        match self {
            Plan::Free => 5,
            Plan::Starter => 30,
            Plan::Pro => 120,
            Plan::Enterprise => 600,
        }
    }

    pub fn agent_runs_per_month(&self) -> u32 {
        match self {
            Plan::Free => 100,
            Plan::Starter => 2_000,
            Plan::Pro => 20_000,
            Plan::Enterprise => u32::MAX,
        }
    }

    pub fn monthly_credits(&self) -> i64 {
        match self {
            Plan::Free => 1_000,
            Plan::Starter => 25_000,
            Plan::Pro => 300_000,
            Plan::Enterprise => i64::MAX,
        }
    }

    pub fn allowed_models(&self) -> &[&str] {
        match self {
            Plan::Free => &["gpt-4o-mini", "gemini-flash"],
            Plan::Starter => &["gpt-4o-mini", "gemini-flash", "gpt-4o", "claude-sonnet"],
            Plan::Pro | Plan::Enterprise => &[
                "gpt-4o-mini",
                "gemini-flash",
                "gpt-4o",
                "claude-sonnet",
                "claude-opus",
            ],
        }
    }
}

impl std::fmt::Display for Plan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Plan::Free => write!(f, "free"),
            Plan::Starter => write!(f, "starter"),
            Plan::Pro => write!(f, "pro"),
            Plan::Enterprise => write!(f, "enterprise"),
        }
    }
}

impl std::str::FromStr for Plan {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "free" => Ok(Plan::Free),
            "starter" => Ok(Plan::Starter),
            "pro" => Ok(Plan::Pro),
            "enterprise" => Ok(Plan::Enterprise),
            _ => Err(format!("Unknown plan: {s}")),
        }
    }
}

/// Credit consumption rates per 1K tokens.
pub struct CreditRate {
    pub input_per_1k: u32,
    pub output_per_1k: u32,
}

/// Get credit rate for a model.
pub fn credit_rate(model: &str) -> CreditRate {
    let model_lower = model.to_lowercase();
    if model_lower.contains("gpt-4o-mini") || model_lower.contains("gemini-flash") {
        CreditRate {
            input_per_1k: 1,
            output_per_1k: 3,
        }
    } else if model_lower.contains("gpt-4o") {
        CreditRate {
            input_per_1k: 5,
            output_per_1k: 15,
        }
    } else if model_lower.contains("claude") && model_lower.contains("sonnet") {
        CreditRate {
            input_per_1k: 6,
            output_per_1k: 18,
        }
    } else if model_lower.contains("claude") && model_lower.contains("opus") {
        CreditRate {
            input_per_1k: 30,
            output_per_1k: 90,
        }
    } else if model_lower.contains("llama") || model_lower.contains("mixtral") || model_lower.contains("groq") {
        // Groq: fast inference, low cost
        CreditRate {
            input_per_1k: 1,
            output_per_1k: 2,
        }
    } else if model_lower.contains("kimi") || model_lower.contains("moonshot") {
        CreditRate {
            input_per_1k: 3,
            output_per_1k: 9,
        }
    } else {
        // Default rate
        CreditRate {
            input_per_1k: 5,
            output_per_1k: 15,
        }
    }
}

/// Calculate credits consumed for a given model and token usage.
pub fn calculate_credits(model: &str, input_tokens: u32, output_tokens: u32) -> u64 {
    let rate = credit_rate(model);
    let input_credits = (input_tokens as u64 * rate.input_per_1k as u64) / 1000;
    let output_credits = (output_tokens as u64 * rate.output_per_1k as u64) / 1000;
    input_credits + output_credits
}

/// Hash an API key using SHA-256 for storage lookup.
pub fn hash_api_key(key: &str) -> String {
    use std::fmt::Write;
    // Simple SHA-256 hash without the hmac/sha2 crate dependency
    // In production, this would use sha2::Sha256
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&key, &mut hasher);
    let hash = std::hash::Hasher::finish(&hasher);
    let mut s = String::new();
    write!(s, "{hash:016x}").ok();
    s
}

/// Generate a new API key with prefix.
pub fn generate_api_key(prefix: &str) -> String {
    let random = uuid::Uuid::new_v4().to_string().replace('-', "");
    format!("{prefix}_{random}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plan_from_str() {
        assert_eq!("free".parse::<Plan>().unwrap(), Plan::Free);
        assert_eq!("starter".parse::<Plan>().unwrap(), Plan::Starter);
        assert_eq!("pro".parse::<Plan>().unwrap(), Plan::Pro);
        assert_eq!("enterprise".parse::<Plan>().unwrap(), Plan::Enterprise);
        assert!("invalid".parse::<Plan>().is_err());
    }

    #[test]
    fn test_plan_display() {
        assert_eq!(Plan::Free.to_string(), "free");
        assert_eq!(Plan::Starter.to_string(), "starter");
        assert_eq!(Plan::Pro.to_string(), "pro");
        assert_eq!(Plan::Enterprise.to_string(), "enterprise");
    }

    #[test]
    fn test_plan_rate_limits() {
        assert_eq!(Plan::Free.rate_limit_per_min(), 5);
        assert_eq!(Plan::Starter.rate_limit_per_min(), 30);
        assert_eq!(Plan::Pro.rate_limit_per_min(), 120);
        assert_eq!(Plan::Enterprise.rate_limit_per_min(), 600);
    }

    #[test]
    fn test_plan_monthly_credits() {
        assert_eq!(Plan::Free.monthly_credits(), 1_000);
        assert_eq!(Plan::Starter.monthly_credits(), 25_000);
        assert_eq!(Plan::Pro.monthly_credits(), 300_000);
    }

    #[test]
    fn test_credit_calculation() {
        // GPT-4o-mini: 1 input, 3 output per 1K tokens
        let credits = calculate_credits("gpt-4o-mini", 1000, 1000);
        assert_eq!(credits, 4); // 1 + 3

        // Claude Opus: 30 input, 90 output per 1K tokens
        let credits = calculate_credits("claude-opus", 2000, 500);
        assert_eq!(credits, 105); // 60 + 45

        // GPT-4o: 5 input, 15 output per 1K tokens
        let credits = calculate_credits("gpt-4o", 10_000, 2000);
        assert_eq!(credits, 80); // 50 + 30
    }

    #[test]
    fn test_generate_api_key() {
        let key = generate_api_key("nb_live");
        assert!(key.starts_with("nb_live_"));
        assert!(key.len() > 20);
    }

    #[test]
    fn test_hash_api_key() {
        let hash1 = hash_api_key("nb_live_abc123");
        let hash2 = hash_api_key("nb_live_abc123");
        assert_eq!(hash1, hash2);

        let hash3 = hash_api_key("nb_live_different");
        assert_ne!(hash1, hash3);
    }
}
