/// Trait for memory storage backends.
pub trait MemoryBackend: Send + Sync {
    /// Read today's memory notes.
    fn read_today(&self) -> String;

    /// Append content to today's memory notes.
    fn append_today(&self, content: &str);

    /// Read long-term memory (MEMORY.md).
    fn read_long_term(&self) -> String;

    /// Write to long-term memory (MEMORY.md).
    fn write_long_term(&self, content: &str);

    /// Get memories from the last N days.
    fn get_recent_memories(&self, days: u32) -> String;

    /// List all memory files sorted by date (newest first).
    fn list_memory_files(&self) -> Vec<std::path::PathBuf>;

    /// Get memory context for the agent (long-term + today's notes).
    fn get_memory_context(&self) -> String;
}
