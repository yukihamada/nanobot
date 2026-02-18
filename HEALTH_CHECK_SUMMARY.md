# Health Check and API Key Rotation Implementation

## Summary

Implemented a comprehensive provider health check and API key rotation system for the chatweb.ai Media API to improve reliability and handle rate limits gracefully.

## Components Implemented

### 1. Provider Health Check System

**Location**: `crates/nanobot-core/src/service/http.rs` (lines 232-400)

**Structures**:
- `ProviderHealth`: Tracks provider health status, latency, success rate, and errors
  - `provider`: Provider name (e.g., "openai-tts", "fal", "kling")
  - `status`: "healthy", "degraded", or "down"
  - `avg_latency_ms`: Average response time
  - `success_rate`: Success rate (0.0-1.0)
  - `last_error`: Most recent error message

**Functions**:
- `record_provider_metric()`: Records latency and success/failure to DynamoDB
  - PK: `PROVIDER_HEALTH#{provider}`
  - SK: `METRIC#{timestamp}`
  - TTL: 5 minutes (auto-cleanup)
  
- `check_provider_health()`: Queries recent metrics and calculates health status
  - Queries last 100 data points (5-minute window)
  - Calculates average latency and success rate
  - Determines status based on thresholds:
    - Healthy: ≥90% success rate, <5s latency
    - Degraded: ≥70% success rate or <10s latency
    - Down: Below degraded thresholds

- `get_fastest_provider()`: Selects the best provider from a list
  - Score formula: `success_rate * 1000 - (latency_ms / 10)`
  - Skips providers marked as "down"
  - Returns the highest-scoring provider

### 2. API Key Rotation System

**Structures**:
- `ApiKeyPool`: Manages multiple API keys per provider with round-robin rotation
  - Reads from plural env vars first (e.g., `OPENAI_API_KEYS`)
  - Falls back to singular env vars (e.g., `OPENAI_API_KEY`)
  - Comma-separated key format: `key1,key2,key3`
  - Thread-safe atomic counter for rotation

**Global Pools**:
- `OPENAI_KEY_POOL`: OpenAI TTS and DALL-E
- `FAL_KEY_POOL`: Flux image generation and Stable Audio
- `KLING_KEY_POOL`: Kling video generation
- `ELEVENLABS_KEY_POOL`: ElevenLabs TTS

**Functions**:
- `get_api_key(provider)`: Gets current API key for provider
- `rotate_key(provider)`: Rotates to next key (useful after 429 errors)
- `mark_key_exhausted(provider, key)`: Logs exhausted keys (can be extended to DynamoDB blacklisting)

### 3. Integration with Media APIs

**Updated Functions**:

#### TTS Handlers
- `try_openai_tts()`: OpenAI TTS with 429 retry
  - Uses `get_api_key("openai")`
  - On 429: marks key exhausted, rotates, and retries
  - Logs latency metrics
  
- `try_elevenlabs_tts()`: ElevenLabs TTS
  - Uses `get_api_key("elevenlabs")`

#### Image Generation
- `handle_media_image()`: DALL-E and Flux
  - DALL-E: Uses `get_api_key("openai")` with 429 retry
  - Flux: Uses `get_api_key("fal")` with 429 retry
  - Both providers: automatic key rotation on rate limits

#### Video Generation
- `poll_kling_video()`: Kling AI
  - Uses `get_api_key("kling")`

#### Audio Generation
- `poll_stable_audio()`: Stable Audio (fal.ai)
  - Uses `get_api_key("fal")`

## Environment Variables

### New Format (Recommended)
```bash
# OpenAI - multiple keys for load distribution
OPENAI_API_KEYS=sk-key1,sk-key2,sk-key3

# fal.ai - multiple keys
FAL_KEYS=key1,key2,key3

# Kling AI - multiple keys
KLING_API_KEYS=key1,key2

# ElevenLabs - multiple keys
ELEVENLABS_API_KEYS=key1,key2
```

### Legacy Format (Still Supported)
```bash
# Single keys (fallback)
OPENAI_API_KEY=sk-key1
FAL_KEY=key1
KLING_API_KEY=key1
ELEVENLABS_API_KEY=key1
```

## Features

### Automatic Rate Limit Handling
- Detects HTTP 429 responses
- Automatically rotates to next available API key
- Retries request with new key
- Logs all rotation events for monitoring

### Health Monitoring (DynamoDB)
- Tracks provider performance in real-time
- 5-minute rolling window with auto-cleanup (TTL)
- Fire-and-forget metric recording (non-blocking)
- Queryable for debugging and alerting

### Graceful Degradation
- Falls back to single keys if plural env vars not set
- Continues with current key if no alternates available
- Clear error messages when all keys exhausted

## Testing

All existing tests pass:
```bash
cargo test -p nanobot-core --lib
# Result: ok. 118 passed; 0 failed; 0 ignored
```

## Deployment Notes

### DynamoDB Table
No schema changes required. Uses existing table with new PK pattern:
- PK: `PROVIDER_HEALTH#{provider}`
- SK: `METRIC#{timestamp_millis}`
- Attributes: `latency_ms`, `success`, `timestamp`, `ttl`

### CloudWatch Logs
Enhanced logging for monitoring:
- `tracing::warn!`: Rate limits detected
- `tracing::info!`: Successful key rotations
- `tracing::error!`: API failures
- All logs include `latency_ms` field for metrics

### Lambda Configuration
No changes required. System is backward compatible:
- Works with single or multiple keys
- No runtime dependencies added
- No Lambda timeout impact (fire-and-forget metrics)

## Future Enhancements

1. **Key Blacklisting**: Store exhausted keys in DynamoDB with TTL
2. **Health Dashboard**: `/api/v1/providers/health` endpoint
3. **Auto-scaling**: Increase concurrency for healthy providers
4. **Alerting**: SNS notifications for degraded providers
5. **Metrics Export**: CloudWatch custom metrics from DynamoDB data

## Files Modified

- `/Users/yuki/workspace/ai/nanobot/crates/nanobot-core/src/service/http.rs`
  - Added: 350+ lines of health check and rotation code
  - Modified: 8 provider functions (TTS, image, video, audio)

## Commit Message

```
Add provider health check and API key rotation system

Implement comprehensive provider health monitoring and automatic API key
rotation to improve reliability and handle rate limits for Media API.

Features:
- Provider health tracking in DynamoDB (5-min rolling window)
- API key pool with round-robin rotation
- Automatic 429 retry with key rotation
- Support for multiple keys per provider (comma-separated env vars)
- Backward compatible with single-key configuration

Integrated providers:
- OpenAI (TTS, DALL-E)
- fal.ai (Flux, Stable Audio)
- Kling AI (video)
- ElevenLabs (TTS)

All tests passing (118/118).

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>
```
