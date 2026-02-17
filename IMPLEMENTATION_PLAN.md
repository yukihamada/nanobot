# Health Check and API Key Rotation System

## Overview
Add provider health monitoring and API key rotation to improve reliability and handle rate limits for Media API providers.

## Architecture

### 1. Provider Health Check System
- **ProviderHealth struct**: Track health status, latency, success rate per provider
- **DynamoDB storage**: PK=PROVIDER_HEALTH#{provider}, sk=METRIC, TTL=5min
- **Functions**:
  - `check_provider_health`: Query recent metrics
  - `get_fastest_provider`: Select best provider based on health
  - `record_provider_metric`: Store request results

### 2. API Key Rotation System
- **ApiKeyPool struct**: Manage multiple keys per provider with round-robin
- **Environment variables**: 
  - Plural form (e.g., `OPENAI_API_KEYS`) comma-separated
  - Fallback to singular (e.g., `OPENAI_API_KEY`)
- **Functions**:
  - `get_api_key`: Get next available key
  - `rotate_key`: Move to next key in pool
  - `mark_key_exhausted`: Skip key temporarily on 429

### 3. Integration Points
- Modify existing handlers:
  - `tts_openai`, `tts_polly`, `tts_qwen3`
  - `generate_image_dalle`, `generate_image_flux`
  - `generate_video_kling`
  - `generate_audio_stable_audio`
- Add health checks before provider selection
- Add key rotation on 429 errors
- Record metrics after each request

## Implementation Steps

1. **Add health check structures** (near top of http.rs)
2. **Add API key rotation structures**
3. **Implement DynamoDB health metric storage**
4. **Implement key pool management**
5. **Integrate into TTS handlers**
6. **Integrate into Image/Video/Audio handlers**
7. **Add monitoring/logging**

## Testing Strategy
- Unit tests for key rotation logic
- Integration tests with mock DynamoDB
- Manual testing with real providers
- Monitor CloudWatch logs for health metrics

## Rollout
- Deploy to dev environment first
- Monitor error rates and latency
- Gradual rollout to production

## Risk Mitigation
- Graceful degradation if DynamoDB unavailable
- Fallback to current behavior if no health data
- Keep existing single-key support for backward compatibility
