# Media API Caching Flow

## Request Flow with Caching

```
┌─────────────────────────────────────────────────────────────────┐
│  User Request: POST /api/v1/media/{tts|image|music|video}      │
│  Body: { prompt, model, voice, etc. }                           │
└────────────────────────┬────────────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────────────┐
│  1. Authentication & Authorization                               │
│     - Extract Bearer token                                       │
│     - Verify user_id                                             │
└────────────────────────┬────────────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────────────┐
│  2. Generate Cache Key                                           │
│     normalize_params(prompt, model, voice, ...) → JSON          │
│     SHA-256(request_type + JSON) → hash                         │
│     cache_key = "CACHE#{type}#{hash}"                           │
└────────────────────────┬────────────────────────────────────────┘
                         │
                         ▼
                    ┌────┴────┐
                    │         │
              CACHE HIT?   CACHE MISS?
                    │         │
        ┌───────────┘         └──────────┐
        │                                 │
        ▼                                 ▼
┌──────────────────┐            ┌──────────────────────┐
│ 3a. Cache Hit    │            │ 3b. Cache Miss       │
│                  │            │                      │
│ - Get cached URL │            │ - Calculate credits  │
│ - Deduct 1 credit│            │ - Deduct full credits│
│ - Increment hits │            │ - Generate content   │
│ - Return result  │            │   (TTS/Image/etc.)   │
│                  │            │ - Upload to S3 (TTS) │
│                  │            │ - Save to cache      │
│                  │            │ - Return result      │
└──────────────────┘            └──────────────────────┘
        │                                 │
        └────────────┬────────────────────┘
                     │
                     ▼
┌─────────────────────────────────────────────────────────────────┐
│  Response                                                        │
│  {                                                               │
│    "url": "https://...",                                        │
│    "credits_used": 1 (cache hit) or N (cache miss),            │
│    "cached": true/false,                                        │
│    "provider": "openai" | "suno" | "kling"                     │
│  }                                                               │
└─────────────────────────────────────────────────────────────────┘
```

## DynamoDB Cache Structure

```
┌──────────────────────────────────────────────────────────────────┐
│  Table: nanobot-config-{env}                                     │
├──────────────────────────────────────────────────────────────────┤
│  PK (String)              │  SK (String)  │  Attributes          │
├───────────────────────────┼───────────────┼──────────────────────┤
│  CACHE#tts#{hash}         │  RESULT       │  result_url: S3 URL  │
│                           │               │  provider: "openai"  │
│                           │               │  credits_used: 2     │
│                           │               │  created_at: RFC3339 │
│                           │               │  hit_count: 15       │
│                           │               │  ttl: 1739577600     │
│                           │               │  request_params: JSON│
├───────────────────────────┼───────────────┼──────────────────────┤
│  CACHE#image#{hash}       │  RESULT       │  result_url: fal.ai  │
│                           │               │  provider: "flux-pro"│
│                           │               │  credits_used: 15    │
│                           │               │  ...                 │
├───────────────────────────┼───────────────┼──────────────────────┤
│  CACHE#music#{hash}       │  RESULT       │  result_url: suno    │
│                           │               │  provider: "suno"    │
│                           │               │  credits_used: 50    │
│                           │               │  ...                 │
└───────────────────────────┴───────────────┴──────────────────────┘
```

## Cost Savings Example

### Without Caching
```
Request 1: "Generate TTS: Hello World" → OpenAI → 2 credits
Request 2: "Generate TTS: Hello World" → OpenAI → 2 credits
Request 3: "Generate TTS: Hello World" → OpenAI → 2 credits
Total: 6 credits
```

### With Caching
```
Request 1: "Generate TTS: Hello World" → OpenAI → 2 credits (cached)
Request 2: "Generate TTS: Hello World" → Cache Hit → 1 credit
Request 3: "Generate TTS: Hello World" → Cache Hit → 1 credit
Total: 4 credits (33% savings)

After 10 requests: 2 + (9 × 1) = 11 credits (vs 20 without cache = 45% savings)
After 100 requests: 2 + (99 × 1) = 101 credits (vs 200 = 49.5% savings)
```

## Cache Key Generation Logic

```rust
// Example: TTS Request
{
  "text": "こんにちは、世界",
  "voice": "nova",
  "engine": "openai",
  "speed": 1.0
}

// Normalization (alphabetically sorted keys)
params = json!({
    "engine": "openai",
    "speed": 1.0,
    "text": "こんにちは、世界",
    "voice": "nova"
}).to_string()

// Hash generation
input = "tts:" + params
hash = SHA256(input) = "a3f2c8b1..."

// Final cache key
cache_key = "CACHE#tts#a3f2c8b1..."
```

## Expiry & Cleanup

```
┌──────────────────────────────────────────────┐
│  Day 0: Content generated & cached           │
│  ttl = now() + 604800 (7 days)              │
├──────────────────────────────────────────────┤
│  Day 1-6: Cache hits return content          │
│  hit_count increments with each access       │
├──────────────────────────────────────────────┤
│  Day 7: TTL expires                          │
│  DynamoDB auto-deletes item                  │
│  Next request = cache miss → regenerate      │
└──────────────────────────────────────────────┘
```
