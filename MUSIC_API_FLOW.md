# Music Generation API Flow Diagram

## Request Flow

```
User Request
    │
    ▼
POST /api/v1/media/music
{
  "prompt": "upbeat music",
  "duration": 30,
  "type": "music"
}
    │
    ▼
┌─────────────────────────────────────────┐
│ handle_media_music()                    │
│ ─────────────────────────────────────── │
│ 1. Authenticate (Bearer token)          │
│ 2. Validate duration (10/30/60)         │
│ 3. Calculate credits (10/20/40)         │
│ 4. Deduct credits from DynamoDB         │
│ 5. Generate music_id (UUID)             │
│ 6. Create job in DynamoDB               │
│ 7. Spawn background task                │
└─────────────────────────────────────────┘
    │
    ├──────────────────────────────────────┐
    │                                      │
    ▼                                      ▼
Return 202 Accepted              tokio::spawn(poll_stable_audio)
{                                        │
  "music_id": "...",                     │
  "status": "queued",                    ▼
  "audio_url": null,          ┌──────────────────────────────┐
  "provider": "stable-audio", │ Background Worker             │
  "duration": 30,             │ ───────────────────────────── │
  "credits_used": 20          │ 1. Submit to fal.ai           │
}                             │    POST /fal-ai/stable-audio  │
                              │    → get request_id           │
                              │                               │
                              │ 2. Update status: processing  │
                              │                               │
                              │ 3. Poll every 10s (max 60x)  │
                              │    GET /requests/{request_id} │
                              │                               │
                              │ 4. Check status:              │
                              │    - IN_QUEUE → continue      │
                              │    - IN_PROGRESS → continue   │
                              │    - COMPLETED → extract URL  │
                              │    - FAILED → mark failed     │
                              │                               │
                              │ 5. Update DynamoDB:           │
                              │    - status: completed        │
                              │    - audio_url: <url>         │
                              └──────────────────────────────┘
```

## Status Check Flow

```
User Request
    │
    ▼
GET /api/v1/media/music/{music_id}
    │
    ▼
┌─────────────────────────────────────────┐
│ handle_media_music_status()             │
│ ─────────────────────────────────────── │
│ 1. Query DynamoDB                       │
│    PK: MUSIC_JOB#{music_id}             │
│    SK: METADATA                         │
│                                         │
│ 2. Return job details                   │
└─────────────────────────────────────────┘
    │
    ▼
Response:
{
  "music_id": "...",
  "status": "completed",
  "audio_url": "https://fal.ai/...",
  "provider": "stable-audio",
  "duration": 30,
  "credits_used": 20,
  "created_at": "2026-02-17T..."
}
```

## DynamoDB State Transitions

```
┌─────────┐
│ queued  │ ← Initial state (handle_media_music)
└────┬────┘
     │
     ▼
┌─────────────┐
│ processing  │ ← After fal.ai submission
└────┬────────┘
     │
     ├─────────────┐
     │             │
     ▼             ▼
┌───────────┐  ┌──────────┐
│ completed │  │  failed  │
└───────────┘  └──────────┘
  (has URL)    (no URL)
```

## fal.ai API Integration

```
Client                      fal.ai Queue API
  │                               │
  ├─ POST /fal-ai/stable-audio ──►│
  │  {                             │
  │    "prompt": "...",            │
  │    "seconds_total": 30,        │
  │    "steps": 100,               │
  │    "cfg_scale": 7.0            │
  │  }                             │
  │                                │
  │◄── Response ───────────────────┤
  │  {                             │
  │    "request_id": "..."         │
  │  }                             │
  │                                │
  │... wait 10s ...                │
  │                                │
  ├─ GET /requests/{request_id} ──►│
  │                                │
  │◄── Response ───────────────────┤
  │  {                             │
  │    "status": "IN_PROGRESS"     │
  │  }                             │
  │                                │
  │... wait 10s ...                │
  │                                │
  ├─ GET /requests/{request_id} ──►│
  │                                │
  │◄── Response ───────────────────┤
  │  {                             │
  │    "status": "COMPLETED",      │
  │    "output": {                 │
  │      "audio_file": {           │
  │        "url": "https://..."    │
  │      }                          │
  │    }                            │
  │  }                             │
  │                                │
```

## Credit Calculation

```
Duration (seconds) → Credits
─────────────────────────────
     10           →    10
     30           →    20
     60           →    40
```

## Error Scenarios

```
1. Invalid Duration
   Request: { "duration": 15 }
   Response: 400 Bad Request
   
2. Insufficient Credits
   User credits: 5
   Required: 10
   Response: 402 Payment Required
   
3. Missing FAL_KEY
   Background worker logs error
   Job marked as failed
   
4. API Timeout (10 minutes)
   60 polling attempts exhausted
   Job marked as failed
   
5. API Error
   fal.ai returns FAILED status
   Job marked as failed
```

## Implementation Files

```
/Users/yuki/workspace/ai/nanobot/crates/nanobot-core/src/service/http.rs
│
├─ Structs (lines 14770-14785)
│  ├─ MediaMusicRequest
│  └─ MediaMusicResponse
│
├─ DynamoDB Helpers (lines 1258-1368)
│  ├─ create_music_job()
│  ├─ update_music_job()
│  └─ get_music_job()
│
├─ HTTP Handlers (lines 16497-16742)
│  ├─ handle_media_music()
│  ├─ poll_stable_audio()
│  └─ handle_media_music_status()
│
└─ Routes (lines 2476-2477)
   ├─ POST /api/v1/media/music
   └─ GET /api/v1/media/music/{id}
```

## Deployment Checklist

- [x] Code implementation
- [x] Compilation check (cargo check)
- [x] Clippy check (cargo clippy)
- [ ] Set FAL_KEY environment variable
- [ ] Deploy to Lambda
- [ ] Test with real API key
- [ ] Monitor CloudWatch logs
- [ ] Update pricing page
- [ ] Update API documentation
