# Media API Request Caching System

## Overview
Implement SHA-256 hash-based caching for TTS, Image, Music, and Video generation requests to avoid regenerating identical content and reduce costs.

## Architecture

### 1. Cache Key Generation
- Use SHA-256 hash of `{request_type}:{normalized_params}`
- Format: `CACHE#{type}#{hash}`
- Normalize parameters to ensure consistent hashing (sorted JSON)

### 2. DynamoDB Schema
```
PK: CACHE#{type}#{hash}
SK: RESULT
Attributes:
- result_url: String (S3 or direct URL)
- provider: String (e.g., "openai", "suno", "kling")
- credits_used: Number (original generation cost)
- created_at: String (ISO 8601)
- hit_count: Number (incremented on cache hits)
- ttl: Number (Unix timestamp, 7 days)
- request_params: String (for debugging)
```

### 3. Cache Module Structure
Create new module: `crates/nanobot-core/src/cache/media_cache.rs`
- `generate_cache_key(request_type, params)` -> String
- `check_cache(cache_key)` -> Option<CachedResult>
- `save_to_cache(cache_key, result_url, provider, credits, params)` -> Result<()>
- `increment_cache_hit(cache_key)` -> Result<()>

### 4. Integration Points

**TTS (`handle_media_tts`)**:
1. Generate cache key from `{text, voice, language, provider}`
2. Check cache → if hit, return cached URL, charge 1 credit
3. If miss, generate TTS, save to cache, charge full credits

**Image (`handle_media_image`)**:
1. Generate cache key from `{prompt, size, quality, provider}`
2. Check cache → if hit, return cached URL, charge 1 credit
3. If miss, generate image, save to cache, charge full credits

**Music (`handle_media_music`)**:
1. Generate cache key from `{prompt, duration, provider}`
2. Check cache → if hit, return cached URL, charge 1 credit
3. If miss, submit job, on completion save to cache, charge full credits

**Video (`handle_media_video`)**:
1. Generate cache key from `{prompt, duration, aspect_ratio, provider}`
2. Check cache → if hit, return cached URL, charge 1 credit
3. If miss, submit job, on completion save to cache, charge full credits

### 5. Credit Model
- Cache hit: 1 credit
- Cache miss: Full generation cost (varies by media type)
- Original credits_used stored for reference

### 6. TTL & Cleanup
- TTL: 7 days (604800 seconds from creation)
- DynamoDB auto-deletes expired items
- No manual cleanup needed

## Implementation Steps

1. Create `cache/media_cache.rs` module with core functions
2. Add cache check/save logic to each media handler
3. Update credit charging logic for cache hits
4. Add logging for cache hits/misses
5. Test with identical requests

## Testing Plan
- Unit tests for cache key generation (consistent hashing)
- Integration test: Generate TTS twice, verify cache hit on second request
- Integration test: Same prompt with different params should miss cache
- Verify credit deduction (full cost on miss, 1 credit on hit)
- Verify TTL is set correctly

## Risks & Mitigations
- **Risk**: Cache key collisions
  - **Mitigation**: SHA-256 provides strong collision resistance
- **Risk**: Stale cached URLs (S3 URLs expiring)
  - **Mitigation**: 7-day TTL shorter than typical S3 signed URL validity
- **Risk**: Parameter normalization issues
  - **Mitigation**: Use deterministic JSON serialization (sorted keys)

## Success Criteria
- Identical requests return cached results within <100ms
- Cache hit rate >30% after 1 week of production use
- No false cache hits (different requests returning wrong content)
- Credit savings visible in metrics
