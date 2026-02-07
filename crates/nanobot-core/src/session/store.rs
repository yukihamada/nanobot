use crate::session::Session;

/// Trait for session storage backends.
pub trait SessionStore: Send + Sync {
    /// Get an existing session or create a new one.
    fn get_or_create(&mut self, key: &str) -> &mut Session;

    /// Save a session.
    fn save(&self, session: &Session);

    /// Save a session by key (looks up in cache).
    fn save_by_key(&self, key: &str);

    /// Delete a session.
    fn delete(&mut self, key: &str) -> bool;

    /// List all sessions.
    fn list_sessions(&self) -> Vec<serde_json::Value>;
}
