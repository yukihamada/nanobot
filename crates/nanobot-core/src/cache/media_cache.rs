//! Media API Request Caching System
//!
//! Implements SHA-256 hash-based caching for TTS, Image, Music, and Video generation
//! requests to avoid regenerating identical content and reduce costs.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[cfg(feature = "dynamodb-backend")]
use aws_sdk_dynamodb::{types::AttributeValue, Client};

/// Cached result from DynamoDB
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedResult {
    pub result_url: String,
    pub provider: String,
    pub credits_used: i64,
    pub created_at: String,
    pub hit_count: i64,
}

/// Generate SHA-256 hash-based cache key from request parameters
///
/// # Arguments
/// * `request_type` - Type of media request (tts, image, music, video)
/// * `params` - JSON string of normalized parameters
///
/// # Returns
/// Cache key in format: `CACHE#{type}#{hash}`
pub fn generate_cache_key(request_type: &str, params: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{}:{}", request_type, params).as_bytes());
    let hash = format!("{:x}", hasher.finalize());
    format!("CACHE#{}#{}", request_type, hash)
}

/// Check if a cached result exists for the given cache key
///
/// # Arguments
/// * `dynamo` - DynamoDB client
/// * `table` - Table name
/// * `cache_key` - Cache key to look up
///
/// # Returns
/// `Some(CachedResult)` if found, `None` otherwise
#[cfg(feature = "dynamodb-backend")]
pub async fn check_cache(
    dynamo: &Client,
    table: &str,
    cache_key: &str,
) -> Option<CachedResult> {
    use std::collections::HashMap;
    let result = dynamo
        .get_item()
        .table_name(table)
        .key("pk", AttributeValue::S(cache_key.to_string()))
        .key("sk", AttributeValue::S("RESULT".to_string()))
        .send()
        .await;

    match result {
        Ok(output) => {
            if let Some(item) = output.item {
                Some(parse_cached_result(&item))
            } else {
                None
            }
        }
        Err(e) => {
            tracing::warn!("Cache lookup failed for {}: {:?}", cache_key, e);
            None
        }
    }
}

/// Save generation result to cache
///
/// # Arguments
/// * `dynamo` - DynamoDB client
/// * `table` - Table name
/// * `cache_key` - Cache key
/// * `result_url` - S3 or direct URL of generated content
/// * `provider` - Provider used (e.g., "openai", "suno", "kling")
/// * `credits` - Credits used for generation
/// * `params` - Original request parameters (for debugging)
#[cfg(feature = "dynamodb-backend")]
pub async fn save_to_cache(
    dynamo: &Client,
    table: &str,
    cache_key: &str,
    result_url: &str,
    provider: &str,
    credits: i64,
    params: &str,
) -> Result<(), String> {
    let now = chrono::Utc::now();
    let ttl = now.timestamp() + 604800; // 7 days in seconds

    let result = dynamo
        .put_item()
        .table_name(table)
        .item("pk", AttributeValue::S(cache_key.to_string()))
        .item("sk", AttributeValue::S("RESULT".to_string()))
        .item("result_url", AttributeValue::S(result_url.to_string()))
        .item("provider", AttributeValue::S(provider.to_string()))
        .item("credits_used", AttributeValue::N(credits.to_string()))
        .item("created_at", AttributeValue::S(now.to_rfc3339()))
        .item("hit_count", AttributeValue::N("0".to_string()))
        .item("ttl", AttributeValue::N(ttl.to_string()))
        .item("request_params", AttributeValue::S(params.to_string()))
        .send()
        .await;

    match result {
        Ok(_) => {
            tracing::info!("Saved to cache: {}", cache_key);
            Ok(())
        }
        Err(e) => {
            tracing::error!("Failed to save cache {}: {:?}", cache_key, e);
            Err(format!("Cache save failed: {:?}", e))
        }
    }
}

/// Increment cache hit counter
///
/// # Arguments
/// * `dynamo` - DynamoDB client
/// * `table` - Table name
/// * `cache_key` - Cache key
#[cfg(feature = "dynamodb-backend")]
pub async fn increment_cache_hit(
    dynamo: &Client,
    table: &str,
    cache_key: &str,
) -> Result<(), String> {
    let result = dynamo
        .update_item()
        .table_name(table)
        .key("pk", AttributeValue::S(cache_key.to_string()))
        .key("sk", AttributeValue::S("RESULT".to_string()))
        .update_expression("ADD hit_count :inc")
        .expression_attribute_values(":inc", AttributeValue::N("1".to_string()))
        .send()
        .await;

    match result {
        Ok(_) => Ok(()),
        Err(e) => {
            tracing::warn!("Failed to increment cache hit for {}: {:?}", cache_key, e);
            Err(format!("Cache hit increment failed: {:?}", e))
        }
    }
}

/// Parse DynamoDB item into CachedResult
#[cfg(feature = "dynamodb-backend")]
fn parse_cached_result(item: &std::collections::HashMap<String, AttributeValue>) -> CachedResult {
    CachedResult {
        result_url: item
            .get("result_url")
            .and_then(|v| v.as_s().ok())
            .unwrap_or(&String::new())
            .to_string(),
        provider: item
            .get("provider")
            .and_then(|v| v.as_s().ok())
            .unwrap_or(&String::from("unknown"))
            .to_string(),
        credits_used: item
            .get("credits_used")
            .and_then(|v| v.as_n().ok())
            .and_then(|n| n.parse().ok())
            .unwrap_or(0),
        created_at: item
            .get("created_at")
            .and_then(|v| v.as_s().ok())
            .unwrap_or(&String::new())
            .to_string(),
        hit_count: item
            .get("hit_count")
            .and_then(|v| v.as_n().ok())
            .and_then(|n| n.parse().ok())
            .unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_cache_key_consistency() {
        let params1 = r#"{"prompt":"hello","voice":"nova"}"#;
        let params2 = r#"{"prompt":"hello","voice":"nova"}"#;
        
        let key1 = generate_cache_key("tts", params1);
        let key2 = generate_cache_key("tts", params2);
        
        assert_eq!(key1, key2, "Identical params should generate identical keys");
    }

    #[test]
    fn test_generate_cache_key_uniqueness() {
        let params1 = r#"{"prompt":"hello","voice":"nova"}"#;
        let params2 = r#"{"prompt":"hello","voice":"alloy"}"#;
        
        let key1 = generate_cache_key("tts", params1);
        let key2 = generate_cache_key("tts", params2);
        
        assert_ne!(key1, key2, "Different params should generate different keys");
    }

    #[test]
    fn test_generate_cache_key_type_separation() {
        let params = r#"{"prompt":"test"}"#;
        
        let key_tts = generate_cache_key("tts", params);
        let key_image = generate_cache_key("image", params);
        
        assert_ne!(key_tts, key_image, "Different types should generate different keys");
    }

    #[test]
    fn test_cache_key_format() {
        let params = r#"{"prompt":"test"}"#;
        let key = generate_cache_key("tts", params);
        
        assert!(key.starts_with("CACHE#tts#"), "Key should start with CACHE#tts#");
        assert_eq!(key.len(), 11 + 64, "Key should be CACHE#tts# + 64 hex chars");
    }
}
