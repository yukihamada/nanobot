# Media API Caching System - Implementation Summary

## Status: Core Module Complete ✓

### Completed Components

#### 1. Cache Module (`crates/nanobot-core/src/cache/`)
- **media_cache.rs**: Core caching logic with SHA-256 hash-based keys
  - `generate_cache_key(type, params)` → `CACHE#{type}#{hash}`
  - `check_cache(dynamo, table, key)` → Option<CachedResult>
  - `save_to_cache(dynamo, table, key, url, provider, credits, params)`
  - `increment_cache_hit(dynamo, table, key)`
  - Unit tests for key generation (consistency, uniqueness, type separation)

#### 2. HTTP Service Integration (`crates/nanobot-core/src/service/http.rs`)
- Added cache module imports
- Added parameter normalization functions:
  - `normalize_tts_params()`
  - `normalize_image_params()`
  - `normalize_music_params()`
  - `normalize_video_params()`

#### 3. Configuration Updates
- Added `sha2` to `dynamodb-backend` feature in Cargo.toml
- Added `cache` module to lib.rs exports

### DynamoDB Schema
```
PK: CACHE#{type}#{sha256_hash}
SK: RESULT
Attributes:
- result_url: String (S3 or direct URL)
- provider: String (openai, suno, kling, etc.)
- credits_used: Number (original generation cost)
- created_at: String (ISO 8601 timestamp)
- hit_count: Number (incremented on each cache hit)
- ttl: Number (Unix timestamp, 7 days from creation)
- request_params: String (JSON, for debugging)
```

### Cache Behavior
- **Cache Hit**: Returns cached URL, charges 1 credit, increments hit_count
- **Cache Miss**: Generates content, charges full credits, saves to cache
- **TTL**: 7 days (604800 seconds), DynamoDB auto-deletes expired items
- **Savings**: 80-95% cost reduction on repeated requests

### Compilation Status
- Core cache module: ✓ Compiles successfully
- Unit tests: ✓ Passing
- HTTP service: Partial (existing unrelated errors in main branch)

## Next Steps

### 1. Fix Existing HTTP Service Errors
The main branch has pre-existing compilation errors that need to be fixed before completing cache integration:
- Duplicate `SK_PROFILE` constant
- `get_user_from_token` function not found
- Type mismatches in `get_or_create_user_cached`

### 2. Complete Handler Integration
Once http.rs compiles, add caching logic to each media handler:

#### TTS Handler (`handle_media_tts`)
```rust
// 1. Generate cache key from request params
// 2. Check cache → if hit, return cached audio URL, charge 1 credit
// 3. If miss, call existing TTS logic
// 4. Upload generated audio to S3
// 5. Save S3 URL to cache
```

#### Image Handler (`handle_media_image`)
```rust
// 1. Generate cache key
// 2. Check cache → if hit, return cached image URL, charge 1 credit
// 3. If miss, generate image
// 4. Save result URL to cache (already provider-hosted, no S3 upload needed)
```

#### Music Handler (`handle_media_music`)
```rust
// 1. Generate cache key
// 2. Check cache → if hit, return cached audio URL, charge 1 credit
// 3. If miss, submit Suno job
// 4. On completion, save result URL to cache
```

#### Video Handler (`handle_media_video`)
```rust
// 1. Generate cache key
// 2. Check cache → if hit, return cached video URL, charge 1 credit
// 3. If miss, submit Kling job
// 4. On completion, save result URL to cache
```

### 3. Testing
- Integration test: Generate identical TTS requests, verify cache hit
- Integration test: Different params should miss cache
- Load test: Verify cache hit rate >30% in production-like scenario
- Credit verification: Confirm 1 credit on hit, full credits on miss

### 4. Monitoring & Metrics
- Add cache hit/miss logging
- Track cache hit rate per media type
- Monitor cost savings
- Alert on abnormally low cache hit rates

## Files Modified
- `crates/nanobot-core/src/cache/mod.rs` (new)
- `crates/nanobot-core/src/cache/media_cache.rs` (new)
- `crates/nanobot-core/src/lib.rs` (added cache module)
- `crates/nanobot-core/Cargo.toml` (added sha2 to dynamodb-backend feature)
- `crates/nanobot-core/src/service/http.rs` (added imports & helpers, handlers pending)

## Risk Mitigation
- SHA-256 provides strong collision resistance
- 7-day TTL shorter than typical S3 signed URL validity
- Deterministic JSON serialization ensures consistent hashing
- Fire-and-forget cache saves don't block user requests
- Cache failures gracefully degrade (continue without caching)
