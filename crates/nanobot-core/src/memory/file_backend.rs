use std::path::{Path, PathBuf};

use crate::util::{ensure_dir, today_date};

use super::backend::MemoryBackend;

/// File-based memory backend.
pub struct FileMemoryBackend {
    memory_dir: PathBuf,
    memory_file: PathBuf,
}

impl FileMemoryBackend {
    pub fn new(workspace: &Path) -> Self {
        let memory_dir = workspace.join("memory");
        ensure_dir(&memory_dir).ok();
        let memory_file = memory_dir.join("MEMORY.md");
        Self {
            memory_dir,
            memory_file,
        }
    }

    /// Get path to today's memory file.
    pub fn today_file(&self) -> PathBuf {
        self.memory_dir.join(format!("{}.md", today_date()))
    }
}

impl MemoryBackend for FileMemoryBackend {
    fn read_today(&self) -> String {
        let path = self.today_file();
        if path.exists() {
            std::fs::read_to_string(&path).unwrap_or_default()
        } else {
            String::new()
        }
    }

    fn append_today(&self, content: &str) {
        let path = self.today_file();
        let new_content = if path.exists() {
            let existing = std::fs::read_to_string(&path).unwrap_or_default();
            format!("{}\n{}", existing, content)
        } else {
            format!("# {}\n\n{}", today_date(), content)
        };
        std::fs::write(&path, new_content).ok();
    }

    fn read_long_term(&self) -> String {
        if self.memory_file.exists() {
            std::fs::read_to_string(&self.memory_file).unwrap_or_default()
        } else {
            String::new()
        }
    }

    fn write_long_term(&self, content: &str) {
        std::fs::write(&self.memory_file, content).ok();
    }

    fn get_recent_memories(&self, days: u32) -> String {
        let today = chrono::Local::now().date_naive();
        let mut memories = Vec::new();

        for i in 0..days {
            let date = today - chrono::Duration::days(i as i64);
            let date_str = date.format("%Y-%m-%d").to_string();
            let file_path = self.memory_dir.join(format!("{}.md", date_str));

            if file_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&file_path) {
                    memories.push(content);
                }
            }
        }

        memories.join("\n\n---\n\n")
    }

    fn list_memory_files(&self) -> Vec<PathBuf> {
        if !self.memory_dir.exists() {
            return Vec::new();
        }

        let mut files: Vec<PathBuf> = std::fs::read_dir(&self.memory_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let name = path.file_name()?.to_str()?;
                // Match YYYY-MM-DD.md pattern
                if name.len() == 13 && name.ends_with(".md") && name.chars().nth(4) == Some('-') {
                    Some(path)
                } else {
                    None
                }
            })
            .collect();

        files.sort_by(|a, b| b.cmp(a));
        files
    }

    fn get_memory_context(&self) -> String {
        let mut parts = Vec::new();

        let long_term = self.read_long_term();
        if !long_term.is_empty() {
            parts.push(format!("## Long-term Memory\n{}", long_term));
        }

        let today = self.read_today();
        if !today.is_empty() {
            parts.push(format!("## Today's Notes\n{}", today));
        }

        parts.join("\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_memory_backend_new() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileMemoryBackend::new(tmp.path());
        assert!(store.memory_dir.exists());
    }

    #[test]
    fn test_long_term_memory() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileMemoryBackend::new(tmp.path());

        assert!(store.read_long_term().is_empty());

        store.write_long_term("User likes cats");
        assert_eq!(store.read_long_term(), "User likes cats");

        store.write_long_term("Updated: User likes cats and dogs");
        assert_eq!(store.read_long_term(), "Updated: User likes cats and dogs");
    }

    #[test]
    fn test_daily_notes() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileMemoryBackend::new(tmp.path());

        assert!(store.read_today().is_empty());

        store.append_today("First note");
        let content = store.read_today();
        assert!(content.contains("First note"));
        assert!(content.contains(&today_date()));

        store.append_today("Second note");
        let content = store.read_today();
        assert!(content.contains("First note"));
        assert!(content.contains("Second note"));
    }

    #[test]
    fn test_today_file_path() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileMemoryBackend::new(tmp.path());
        let path = store.today_file();
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert!(filename.ends_with(".md"));
        assert_eq!(filename.len(), 13); // YYYY-MM-DD.md
    }

    #[test]
    fn test_get_memory_context() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileMemoryBackend::new(tmp.path());

        // Empty initially
        assert!(store.get_memory_context().is_empty());

        // Add long-term memory
        store.write_long_term("Important fact");
        let ctx = store.get_memory_context();
        assert!(ctx.contains("Long-term Memory"));
        assert!(ctx.contains("Important fact"));

        // Add daily note
        store.append_today("Today's observation");
        let ctx = store.get_memory_context();
        assert!(ctx.contains("Long-term Memory"));
        assert!(ctx.contains("Today's Notes"));
    }

    #[test]
    fn test_list_memory_files() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileMemoryBackend::new(tmp.path());

        // Create some date-formatted files
        std::fs::write(store.memory_dir.join("2024-01-15.md"), "day1").unwrap();
        std::fs::write(store.memory_dir.join("2024-01-16.md"), "day2").unwrap();
        std::fs::write(store.memory_dir.join("MEMORY.md"), "long term").unwrap();

        let files = store.list_memory_files();
        assert_eq!(files.len(), 2); // Should not include MEMORY.md
    }
}
