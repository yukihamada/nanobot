use serde::{Deserialize, Serialize};

use crate::service::auth::calculate_credits;

/// Usage record for a single agent run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRecord {
    pub tenant_id: String,
    pub timestamp: String,
    pub model: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub credits_used: u64,
    pub session_key: Option<String>,
}

/// Aggregated usage for a period.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageSummary {
    pub agent_runs: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_credits_used: u64,
}

/// Trait for usage tracking backends.
pub trait UsageTracker: Send + Sync {
    /// Record a usage event after an agent run.
    fn record_usage(
        &self,
        tenant_id: &str,
        model: &str,
        input_tokens: u32,
        output_tokens: u32,
        session_key: Option<&str>,
    );

    /// Get usage summary for the current billing period.
    fn get_usage(&self, tenant_id: &str) -> UsageSummary;

    /// Check if a tenant has remaining quota.
    fn check_quota(&self, tenant_id: &str, credits_needed: u64) -> bool;
}

/// In-memory usage tracker (for local dev/testing).
pub struct InMemoryUsageTracker {
    records: std::sync::Mutex<Vec<UsageRecord>>,
}

impl InMemoryUsageTracker {
    pub fn new() -> Self {
        Self {
            records: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl Default for InMemoryUsageTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl UsageTracker for InMemoryUsageTracker {
    fn record_usage(
        &self,
        tenant_id: &str,
        model: &str,
        input_tokens: u32,
        output_tokens: u32,
        session_key: Option<&str>,
    ) {
        let credits = calculate_credits(model, input_tokens, output_tokens);
        let record = UsageRecord {
            tenant_id: tenant_id.to_string(),
            timestamp: crate::util::timestamp(),
            model: model.to_string(),
            input_tokens,
            output_tokens,
            credits_used: credits,
            session_key: session_key.map(|s| s.to_string()),
        };

        if let Ok(mut records) = self.records.lock() {
            records.push(record);
        }
    }

    fn get_usage(&self, tenant_id: &str) -> UsageSummary {
        let records = match self.records.lock() {
            Ok(r) => r,
            Err(_) => return UsageSummary::default(),
        };

        let mut summary = UsageSummary::default();
        for record in records.iter() {
            if record.tenant_id == tenant_id {
                summary.agent_runs += 1;
                summary.total_input_tokens += record.input_tokens as u64;
                summary.total_output_tokens += record.output_tokens as u64;
                summary.total_credits_used += record.credits_used;
            }
        }
        summary
    }

    fn check_quota(&self, _tenant_id: &str, _credits_needed: u64) -> bool {
        // In-memory tracker always allows (no persistent limits)
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_in_memory_usage_tracker() {
        let tracker = InMemoryUsageTracker::new();

        tracker.record_usage("tenant1", "gpt-4o-mini", 1000, 500, Some("session1"));
        tracker.record_usage("tenant1", "gpt-4o", 2000, 1000, None);
        tracker.record_usage("tenant2", "claude-sonnet", 500, 200, None);

        let summary1 = tracker.get_usage("tenant1");
        assert_eq!(summary1.agent_runs, 2);
        assert_eq!(summary1.total_input_tokens, 3000);
        assert_eq!(summary1.total_output_tokens, 1500);

        let summary2 = tracker.get_usage("tenant2");
        assert_eq!(summary2.agent_runs, 1);
    }

    #[test]
    fn test_check_quota() {
        let tracker = InMemoryUsageTracker::new();
        assert!(tracker.check_quota("tenant1", 100));
    }

    #[test]
    fn test_usage_record_credits() {
        let tracker = InMemoryUsageTracker::new();
        tracker.record_usage("t1", "gpt-4o-mini", 10_000, 5_000, None);

        let summary = tracker.get_usage("t1");
        // gpt-4o-mini: 1 input/1K + 3 output/1K = 10 + 15 = 25
        assert_eq!(summary.total_credits_used, 25);
    }
}
