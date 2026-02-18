#[cfg(feature = "dynamodb-backend")]
pub mod media_cache;

#[cfg(feature = "dynamodb-backend")]
pub use media_cache::{
    CachedResult,
    generate_cache_key,
    check_cache,
    save_to_cache,
    increment_cache_hit,
};
