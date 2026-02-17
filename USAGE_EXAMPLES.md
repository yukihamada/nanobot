# Health Check and API Key Rotation - Usage Examples

## Configuration Examples

### Single Key (Legacy, Still Supported)
```bash
# Lambda environment variables
OPENAI_API_KEY=sk-proj-abc123...
FAL_KEY=12345678:abc...
KLING_API_KEY=kling-api-key-123
ELEVENLABS_API_KEY=el-api-key-456
```

### Multiple Keys (Recommended for Production)
```bash
# Comma-separated keys for automatic rotation
OPENAI_API_KEYS=sk-proj-abc123...,sk-proj-def456...,sk-proj-ghi789...
FAL_KEYS=12345:abc...,67890:def...,54321:ghi...
KLING_API_KEYS=kling-key-1,kling-key-2
ELEVENLABS_API_KEYS=el-key-1,el-key-2,el-key-3
```

## How It Works

### 1. Normal Request Flow
```
User → API → get_api_key("openai") → Returns key1
              ↓
         API Request with key1
              ↓
         Success (200 OK)
              ↓
         Return response to user
```

### 2. Rate Limited Request with Auto-Retry
```
User → API → get_api_key("openai") → Returns key1
              ↓
         API Request with key1
              ↓
         Rate Limited (429)
              ↓
         mark_key_exhausted("openai", key1)
              ↓
         rotate_key("openai") → Returns key2
              ↓
         Retry API Request with key2
              ↓
         Success (200 OK)
              ↓
         Return response to user
```

### 3. Health Tracking (Background)
```
API Request → Record start time
              ↓
         Send request
              ↓
         Record end time
              ↓
         Calculate latency_ms
              ↓
    (Fire-and-forget to DynamoDB)
         record_provider_metric()
              ↓
         PK: PROVIDER_HEALTH#openai-tts
         SK: METRIC#1709876543210
         latency_ms: 850
         success: true
         ttl: 1709876843 (5 min later)
```

## API Usage Examples

### TTS Request (OpenAI)
```bash
curl -X POST https://api.chatweb.ai/api/v1/media/tts \
  -H "Authorization: Bearer user-token" \
  -H "Content-Type: application/json" \
  -d '{
    "text": "こんにちは、世界！",
    "voice": "nova",
    "speed": 1.0
  }'
```

**Behind the scenes**:
1. `get_api_key("openai")` → Gets key from pool
2. Request to OpenAI TTS API
3. If 429: Auto-rotate and retry
4. Return MP3 audio data

### Image Generation (DALL-E)
```bash
curl -X POST https://api.chatweb.ai/api/v1/media/image \
  -H "Authorization: Bearer user-token" \
  -H "Content-Type: application/json" \
  -d '{
    "prompt": "A serene Japanese garden at sunset",
    "model": "dalle-3",
    "size": "1024x1024",
    "quality": "hd"
  }'
```

**Response**:
```json
{
  "images": [
    {
      "url": "https://oaidalleapiprodscus.blob.core.windows.net/..."
    }
  ],
  "model": "dalle-3",
  "credits_used": 20
}
```

### Image Generation (Flux via fal.ai)
```bash
curl -X POST https://api.chatweb.ai/api/v1/media/image \
  -H "Authorization: Bearer user-token" \
  -H "Content-Type: application/json" \
  -d '{
    "prompt": "A futuristic city at night",
    "model": "flux-pro",
    "size": "1792x1024"
  }'
```

**Behind the scenes**:
1. `get_api_key("fal")` → Gets fal.ai key
2. Request to fal.ai Flux API
3. If 429: Auto-rotate and retry with different fal.ai key
4. Return generated image URLs

## Monitoring Examples

### Check Provider Health (DynamoDB Query)
```bash
aws dynamodb query \
  --table-name nanobot-config \
  --key-condition-expression "pk = :pk AND begins_with(sk, :sk)" \
  --expression-attribute-values '{
    ":pk": {"S": "PROVIDER_HEALTH#openai-tts"},
    ":sk": {"S": "METRIC#"}
  }' \
  --scan-index-forward false \
  --limit 10
```

**Response**:
```json
{
  "Items": [
    {
      "pk": {"S": "PROVIDER_HEALTH#openai-tts"},
      "sk": {"S": "METRIC#1709876543210"},
      "latency_ms": {"N": "850"},
      "success": {"BOOL": true},
      "timestamp": {"S": "2024-03-08T10:15:43Z"},
      "ttl": {"N": "1709876843"}
    },
    ...
  ]
}
```

### CloudWatch Logs Insights Query
```
fields @timestamp, provider, latency_ms, status
| filter @message like /OpenAI TTS/
| stats avg(latency_ms) as avg_latency, count(*) as requests by bin(5m)
```

### Monitor Rate Limits
```
fields @timestamp, provider, @message
| filter @message like /rate limited/
| stats count(*) as rate_limit_events by provider
```

### Track Key Rotations
```
fields @timestamp, provider, @message
| filter @message like /rotated/
| display @timestamp, provider, @message
```

## Health Check Function Usage (Programmatic)

### In Code (Future Enhancement)
```rust
// Example: Select best TTS provider based on health
let providers = ["openai-tts", "elevenlabs", "polly"];
let best = get_fastest_provider(&dynamo, &table, &providers).await;

match best {
    Some(provider) => {
        tracing::info!("Using {} based on health metrics", provider);
        // Use the selected provider
    }
    None => {
        tracing::warn!("All TTS providers degraded, using fallback");
        // Use fallback
    }
}
```

### Check Individual Provider Health
```rust
let health = check_provider_health(&dynamo, &table, "openai-tts").await;
println!("Provider: {}", health.provider);
println!("Status: {}", health.status);
println!("Avg Latency: {}ms", health.avg_latency_ms);
println!("Success Rate: {:.1}%", health.success_rate * 100.0);
```

**Output**:
```
Provider: openai-tts
Status: healthy
Avg Latency: 850ms
Success Rate: 98.5%
```

## Testing Locally

### Test Key Rotation
```bash
# Set multiple keys
export OPENAI_API_KEYS="sk-test-1,sk-test-2,sk-test-3"

# Run the service
cargo run -p nanobot-lambda

# Make requests - watch logs for rotation on rate limits
```

### Simulate Rate Limit
```rust
// In try_openai_tts function, temporarily force rotation:
if true { // Simulate 429
    mark_key_exhausted("openai", &api_key);
    if let Some(new_key) = rotate_key("openai") {
        tracing::info!("Testing rotation: {} -> {}", 
            &api_key[..8], &new_key[..8]);
    }
}
```

## Production Recommendations

### 1. Key Pool Sizing
- **OpenAI**: 3-5 keys (based on expected TTS/DALL-E volume)
- **fal.ai**: 2-3 keys (Flux image generation burst traffic)
- **Kling AI**: 2 keys (video generation is slower, less concurrent)
- **ElevenLabs**: 2-3 keys (backup TTS, lower volume)

### 2. Monitoring Alerts
Set up CloudWatch alarms for:
- Rate limit frequency > 10/hour per provider
- Average latency > 5000ms
- Success rate < 90%

### 3. Key Management
- Store keys in AWS Secrets Manager
- Rotate keys quarterly
- Use separate keys for dev/staging/production
- Monitor key usage in provider dashboards

### 4. Cost Optimization
- Use multiple keys to stay within free tiers
- Monitor credit consumption per key
- Implement key-level budgets

## Troubleshooting

### Issue: All keys exhausted
**Symptom**: `rate limited, no alternate keys` errors

**Solution**:
1. Add more keys to the pool
2. Implement backoff/retry logic in clients
3. Cache responses where possible
4. Consider upgrading provider tier

### Issue: Keys rotating too frequently
**Symptom**: Constant rotation logs

**Solution**:
1. Check if one key is invalid (remove from pool)
2. Verify rate limits at provider dashboard
3. Reduce concurrent requests
4. Implement request queuing

### Issue: Health metrics not appearing
**Symptom**: No DynamoDB entries

**Solution**:
1. Verify DynamoDB table permissions
2. Check Lambda execution role has `dynamodb:PutItem`
3. Verify `CONFIG_TABLE` environment variable
4. Check CloudWatch logs for errors
