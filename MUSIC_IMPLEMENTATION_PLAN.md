# Music Generation API Implementation Plan

## Overview
Add /api/v1/media/music endpoint to support music and sound effects generation via Stable Audio (fal.ai).

## Changes Required

### 1. Data Structures (after line 14767)
- `MediaMusicRequest`: Request struct with prompt, type_, duration
- `MediaMusicResponse`: Response struct with audio_url, provider, duration, credits_used

### 2. Helper Functions (after line 1240)
- `create_music_job()`: Store music generation job in DynamoDB
- `update_music_job()`: Update job status and URL
- `get_music_job()`: Retrieve job from DynamoDB

### 3. Main Handler (after line 16373)
- `handle_media_music()`: Main endpoint handler
  - Authenticate user
  - Calculate credits (10s=10, 30s=20, 60s=40)
  - Deduct credits
  - Generate job ID
  - Store job in DynamoDB
  - Start background task
  - Return 202 Accepted

### 4. Background Worker (after handle_media_music)
- `poll_stable_audio()`: Background task to poll Stable Audio API
  - Submit generation request to fal.ai
  - Poll for completion
  - Update DynamoDB on completion/failure
  - 60 attempts with 10s intervals (10 minute timeout)

### 5. Status Endpoint (after poll_stable_audio)
- `handle_media_music_status()`: GET /api/v1/media/music/{id}
  - Fetch job from DynamoDB
  - Return job status

### 6. Route Registration (line 2368)
- Add POST /api/v1/media/music
- Add GET /api/v1/media/music/{id}

## API Details

### Stable Audio via fal.ai
- Endpoint: https://queue.fal.run/fal-ai/stable-audio
- Model: stable-audio-open-1.0
- Auth: Bearer token (FAL_KEY env var)
- Duration: 10, 30, or 60 seconds
- Queue-based API (submit → poll → download)

### Credit Pricing
- 10 seconds: 10 credits
- 30 seconds: 20 credits  
- 60 seconds: 40 credits

### DynamoDB Schema
```
pk: MUSIC_JOB#{music_id}
sk: METADATA
user_id: string
prompt: string
provider: string (stable-audio)
type: string (music | sfx)
duration: number
status: string (queued | processing | completed | failed)
url: string (optional)
credits_used: number
created_at: timestamp
updated_at: timestamp
ttl: number (7 days)
```

## Testing
1. Verify FAL_KEY is set
2. Test music generation (10s, 30s, 60s)
3. Test SFX generation
4. Test status polling
5. Test credit deduction
6. Test error handling (no credits, invalid params)
