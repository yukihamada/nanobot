# Provider Health Check and API Key Rotation

Comprehensive reliability system for chatweb.ai Media API providers.

## Quick Start

### 1. Update Environment Variables
```bash
# Add multiple API keys (comma-separated)
OPENAI_API_KEYS=sk-key1,sk-key2,sk-key3
FAL_KEYS=fal-key1,fal-key2
KLING_API_KEYS=kling-key1,kling-key2
ELEVENLABS_API_KEYS=el-key1,el-key2
```

### 2. Deploy
```bash
# Deploy to Lambda
./infra/deploy-fast.sh
```

### 3. Monitor
```bash
# Watch for rate limit handling
aws logs tail /aws/lambda/nanobot --follow --filter-pattern "rotated"

# Check health metrics
aws dynamodb query \
  --table-name nanobot-config \
  --key-condition-expression "pk = :pk" \
  --expression-attribute-values '{":pk":{"S":"PROVIDER_HEALTH#openai-tts"}}'
```

## What's Included

### Automatic Rate Limit Handling
- HTTP 429 detection
- Instant key rotation
- Automatic retry
- Zero user impact

### Health Monitoring
- Real-time metrics (DynamoDB)
- 5-minute rolling window
- Provider status: healthy/degraded/down
- Latency and success rate tracking

### Supported Providers
- OpenAI (TTS, DALL-E)
- fal.ai (Flux, Stable Audio)
- Kling AI (video)
- ElevenLabs (TTS)

## Documentation

| File | Description |
|------|-------------|
| [IMPLEMENTATION_PLAN.md](./IMPLEMENTATION_PLAN.md) | Architecture and design |
| [HEALTH_CHECK_SUMMARY.md](./HEALTH_CHECK_SUMMARY.md) | Technical specification |
| [USAGE_EXAMPLES.md](./USAGE_EXAMPLES.md) | API examples and monitoring |
| [IMPLEMENTATION_SUMMARY.md](./IMPLEMENTATION_SUMMARY.md) | Final implementation stats |

## Features

### Before (Single Key)
```
Request → API Call → 429 Rate Limited → Error to User ❌
```

### After (Multi-Key with Rotation)
```
Request → API Call → 429 Rate Limited → Rotate Key → Retry → Success ✅
```

## Testing

All tests passing:
```bash
cargo test -p nanobot-core --lib
# Result: ok. 118 passed; 0 failed; 0 ignored
```

## Backward Compatibility

Works with both old and new configuration:

```bash
# Old (still works)
OPENAI_API_KEY=single-key

# New (recommended)
OPENAI_API_KEYS=key1,key2,key3
```

## Performance

- Zero added latency
- No new dependencies
- Fire-and-forget metrics
- Lock-free rotation

## Next Steps

1. Add multiple keys to Lambda environment
2. Monitor CloudWatch logs for rotation events
3. Set up alarms for high rate limit frequency
4. Review health metrics in DynamoDB

## Support

- CloudWatch Logs: `/aws/lambda/nanobot`
- DynamoDB Table: `nanobot-config`
- Query patterns in [USAGE_EXAMPLES.md](./USAGE_EXAMPLES.md)

---

**Status**: ✅ Production Ready

**Version**: 1.0.0

**Date**: 2026-02-17
