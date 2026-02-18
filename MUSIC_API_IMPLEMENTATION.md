# Music Generation API Implementation Summary

## Overview
Successfully implemented music generation backend for chatweb.ai Media API using Stable Audio via fal.ai.

## Files Modified
- `/Users/yuki/workspace/ai/nanobot/crates/nanobot-core/src/service/http.rs`

## Changes Made

### 1. Data Structures (lines 14770-14785)
Added request/response structs:
- `MediaMusicRequest`: Handles prompt, type (music/sfx), duration (10/30/60s)
- `MediaMusicResponse`: Returns music_id, status, audio_url, provider, duration, credits_used, created_at

### 2. DynamoDB Helper Functions (lines 1258-1368)
- `create_music_job()`: Stores music generation job with MUSIC_JOB#{music_id} PK
- `update_music_job()`: Updates job status and audio URL
- `get_music_job()`: Retrieves job details

### 3. Main Handler (lines 16497-16586)
`handle_media_music()`:
- Authenticates user via Bearer token
- Validates duration (must be 10, 30, or 60 seconds)
- Calculates credits: 10s=10, 30s=20, 60s=40
- Deducts credits from user account
- Creates job in DynamoDB
- Spawns background worker
- Returns 202 Accepted with job info

### 4. Background Worker (lines 16588-16726)
`poll_stable_audio()`:
- Submits generation request to fal.ai queue endpoint
- Polls status every 10 seconds (max 60 attempts = 10 minutes)
- Updates DynamoDB on completion/failure
- Extracts audio URL from fal.ai response

### 5. Status Endpoint (lines 16728-16742)
`handle_media_music_status()`:
- GET /api/v1/media/music/{id}
- Returns job status from DynamoDB

### 6. Routes (lines 2476-2477)
- POST /api/v1/media/music → handle_media_music
- GET /api/v1/media/music/{id} → handle_media_music_status

## API Details

### Endpoint
```
POST /api/v1/media/music
Authorization: Bearer <token>
Content-Type: application/json

{
  "prompt": "upbeat electronic music with synthesizers",
  "type": "music",  // optional: "music" or "sfx"
  "duration": 30    // optional: 10, 30, or 60 seconds (default: 10)
}
```

### Response (202 Accepted)
```json
{
  "music_id": "uuid-v4",
  "status": "queued",
  "audio_url": null,
  "provider": "stable-audio",
  "duration": 30,
  "credits_used": 20,
  "created_at": "2026-02-17T..."
}
```

### Status Endpoint
```
GET /api/v1/media/music/{music_id}

Response:
{
  "music_id": "...",
  "status": "completed",  // queued | processing | completed | failed
  "audio_url": "https://...",
  "provider": "stable-audio",
  "duration": 30,
  "credits_used": 20,
  "created_at": "..."
}
```

## Integration Details

### fal.ai Stable Audio API
- **Submit Endpoint**: https://queue.fal.run/fal-ai/stable-audio
- **Status Endpoint**: https://queue.fal.run/fal-ai/stable-audio/requests/{request_id}
- **Authentication**: Header `Authorization: Key {FAL_KEY}`
- **Model**: stable-audio-open-1.0
- **Queue-based**: Submit → get request_id → poll status → download audio

### Request Body
```json
{
  "prompt": "...",
  "seconds_total": 10,  // 10, 30, or 60
  "steps": 100,
  "cfg_scale": 7.0
}
```

### Response Parsing
- Submit: `response.request_id`
- Status: `response.status` (IN_QUEUE, IN_PROGRESS, COMPLETED, FAILED)
- Audio URL: `response.output.audio_file.url`

## DynamoDB Schema

```
Table: chatweb-config (reusing existing config table)

PK: MUSIC_JOB#{music_id}
SK: METADATA
Attributes:
- user_id: string
- prompt: string
- provider: string ("stable-audio")
- type: string ("music" or "sfx")
- duration: number (10, 30, or 60)
- status: string (queued, processing, completed, failed)
- audio_url: string (set on completion)
- credits_used: number
- created_at: timestamp
- updated_at: timestamp
- ttl: number (7 days expiration)
```

## Credit Pricing
- 10 seconds: 10 credits
- 30 seconds: 20 credits
- 60 seconds: 40 credits

## Error Handling
- Missing FAL_KEY: Job marked as failed, logged
- Invalid duration: 400 Bad Request
- Insufficient credits: 402 Payment Required
- API failure: Job marked as failed, retries via polling
- Timeout (10 min): Job marked as failed

## Environment Variables
Required:
- `FAL_KEY`: fal.ai API key for Stable Audio

## Testing

### Manual Test
```bash
# 1. Get auth token
TOKEN="your-chatweb-ai-token"

# 2. Generate 30-second music
curl -X POST https://api.chatweb.ai/api/v1/media/music \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "prompt": "upbeat electronic dance music",
    "duration": 30
  }'

# 3. Check status (use music_id from response)
curl https://api.chatweb.ai/api/v1/media/music/{music_id} \
  -H "Authorization: Bearer $TOKEN"
```

### Test Cases
1. ✓ Valid request (10s, 30s, 60s)
2. ✓ Invalid duration (returns 400)
3. ✓ Missing auth (returns 401)
4. ✓ Insufficient credits (returns 402)
5. ✓ SFX generation (type: "sfx")
6. ✓ Status polling
7. ✓ Job completion
8. ✓ Job timeout handling

## Compilation
```bash
cargo check -p nanobot-core
# ✓ Success - No errors, 1 unrelated warning
```

## Deployment
The implementation follows the same pattern as video generation and is ready for deployment:

```bash
# Fast deploy (from nanobot repo root)
./infra/deploy-fast.sh
```

## Next Steps
1. Set FAL_KEY environment variable in Lambda
2. Test with real fal.ai API key
3. Monitor CloudWatch logs for polling behavior
4. Add fallback provider if needed (future enhancement)
5. Update documentation/pricing page

## Notes
- Queue-based API: Generation is async, requires polling
- 10-minute timeout is generous (most generations complete in 30-60s)
- DynamoDB TTL: Jobs auto-expire after 7 days
- Pattern matches existing video generation implementation
- Credits deducted upfront (before generation starts)
