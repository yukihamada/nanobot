/// Database abstraction layer for nanobot.
///
/// Backends:
/// - `DynamoDB` (feature `dynamodb-backend`) — existing AWS infrastructure
/// - `LibSQL`   (feature `libsql-backend`)   — SQLite / Turso (Fly.io, self-host)
pub mod backend;

#[cfg(feature = "libsql-backend")]
pub mod libsql;

pub use backend::{
    AbEvent, ApiKeyRecord, AuditEntry, CouponRecord, DbBackend, HourlyStats, InstalledSkill,
    ProviderMetric, RateLimitResult, SharedConversation, SkillRecord, UserProfile,
};

#[cfg(feature = "libsql-backend")]
pub use self::libsql::LibSqlBackend;
