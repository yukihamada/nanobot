use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::warn;

use crate::config;
use crate::util::safe_filename;

use super::store::SessionStore;
use super::{Session, SessionMessage};

/// File-based session store using JSONL files.
pub struct FileSessionStore {
    sessions_dir: PathBuf,
    cache: HashMap<String, Session>,
}

impl FileSessionStore {
    pub fn new(_workspace: &Path) -> Self {
        let sessions_dir = config::get_data_dir().join("sessions");
        std::fs::create_dir_all(&sessions_dir).ok();
        Self {
            sessions_dir,
            cache: HashMap::new(),
        }
    }

    fn session_path(&self, key: &str) -> PathBuf {
        let safe_key = safe_filename(&key.replace(':', "_"));
        self.sessions_dir.join(format!("{}.jsonl", safe_key))
    }

    /// Load a session from disk.
    fn load(&self, key: &str) -> Option<Session> {
        let path = self.session_path(key);
        if !path.exists() {
            return None;
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to read session {}: {}", key, e);
                return None;
            }
        };

        let mut messages = Vec::new();
        let mut metadata = HashMap::new();
        let mut created_at = None;

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            if let Ok(data) = serde_json::from_str::<serde_json::Value>(line) {
                if data.get("_type").and_then(|v| v.as_str()) == Some("metadata") {
                    if let Some(ca) = data.get("created_at").and_then(|v| v.as_str()) {
                        created_at = chrono::DateTime::parse_from_rfc3339(ca)
                            .ok()
                            .map(|dt| dt.with_timezone(&chrono::Utc));
                    }
                    if let Some(meta) = data.get("metadata") {
                        if let Ok(m) = serde_json::from_value(meta.clone()) {
                            metadata = m;
                        }
                    }
                } else if let Ok(msg) = serde_json::from_value::<SessionMessage>(data) {
                    messages.push(msg);
                }
            }
        }

        Some(Session {
            key: key.to_string(),
            messages,
            created_at: created_at.unwrap_or_else(chrono::Utc::now),
            updated_at: chrono::Utc::now(),
            metadata,
        })
    }
}

impl SessionStore for FileSessionStore {
    fn get_or_create(&mut self, key: &str) -> &mut Session {
        if !self.cache.contains_key(key) {
            let session = self.load(key).unwrap_or_else(|| Session::new(key));
            self.cache.insert(key.to_string(), session);
        }
        self.cache.get_mut(key).unwrap()
    }

    fn save(&self, session: &Session) {
        let path = self.session_path(&session.key);
        let mut lines = Vec::with_capacity(session.messages.len() + 1);

        // Metadata line
        let meta = serde_json::json!({
            "_type": "metadata",
            "created_at": session.created_at.to_rfc3339(),
            "updated_at": session.updated_at.to_rfc3339(),
            "metadata": session.metadata,
        });
        lines.push(serde_json::to_string(&meta).unwrap_or_default());

        // Message lines
        for msg in &session.messages {
            if let Ok(line) = serde_json::to_string(msg) {
                lines.push(line);
            }
        }

        let content = lines.join("\n") + "\n";
        if let Err(e) = std::fs::write(&path, content) {
            warn!("Failed to save session {}: {}", session.key, e);
        }
    }

    fn save_by_key(&self, key: &str) {
        if let Some(session) = self.cache.get(key) {
            self.save(session);
        }
    }

    fn delete(&mut self, key: &str) -> bool {
        self.cache.remove(key);
        let path = self.session_path(key);
        if path.exists() {
            std::fs::remove_file(&path).is_ok()
        } else {
            false
        }
    }

    fn list_sessions(&self) -> Vec<serde_json::Value> {
        let mut sessions = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.sessions_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        if let Some(first_line) = content.lines().next() {
                            if let Ok(data) = serde_json::from_str::<serde_json::Value>(first_line)
                            {
                                if data.get("_type").and_then(|v| v.as_str()) == Some("metadata") {
                                    let key = path
                                        .file_stem()
                                        .and_then(|s| s.to_str())
                                        .unwrap_or("")
                                        .replace('_', ":");
                                    sessions.push(serde_json::json!({
                                        "key": key,
                                        "created_at": data.get("created_at"),
                                        "updated_at": data.get("updated_at"),
                                        "path": path.display().to_string(),
                                    }));
                                }
                            }
                        }
                    }
                }
            }
        }
        sessions.sort_by(|a, b| {
            let ua = a.get("updated_at").and_then(|v| v.as_str()).unwrap_or("");
            let ub = b.get("updated_at").and_then(|v| v.as_str()).unwrap_or("");
            ub.cmp(ua)
        });
        sessions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_session_store_create_and_save() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = FileSessionStore::new(tmp.path());

        // Override sessions_dir to use temp
        store.sessions_dir = tmp.path().join("sessions");
        std::fs::create_dir_all(&store.sessions_dir).unwrap();

        let session = store.get_or_create("test:abc");
        session.add_message("user", "Hello");
        session.add_message("assistant", "Hi!");

        store.save_by_key("test:abc");

        // Verify the file was written
        let path = store.session_path("test:abc");
        assert!(path.exists());

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("metadata"));
        assert!(content.contains("Hello"));
        assert!(content.contains("Hi!"));
    }

    #[test]
    fn test_file_session_store_load() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = FileSessionStore::new(tmp.path());
        store.sessions_dir = tmp.path().join("sessions");
        std::fs::create_dir_all(&store.sessions_dir).unwrap();

        // Create and save
        let session = store.get_or_create("load:test");
        session.add_message("user", "persistent msg");
        store.save_by_key("load:test");

        // Clear cache and reload
        store.cache.clear();
        let session = store.get_or_create("load:test");
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].content, "persistent msg");
    }

    #[test]
    fn test_file_session_store_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = FileSessionStore::new(tmp.path());
        store.sessions_dir = tmp.path().join("sessions");
        std::fs::create_dir_all(&store.sessions_dir).unwrap();

        let session = store.get_or_create("del:test");
        session.add_message("user", "temp");
        store.save_by_key("del:test");

        let path = store.session_path("del:test");
        assert!(path.exists());

        assert!(store.delete("del:test"));
        assert!(!path.exists());
    }
}
