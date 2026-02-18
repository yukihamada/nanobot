use std::path::{Path, PathBuf};

/// Default interval: 30 minutes.
pub const DEFAULT_HEARTBEAT_INTERVAL_S: u64 = 30 * 60;

/// The prompt sent to agent during heartbeat.
pub const HEARTBEAT_PROMPT: &str = "Read HEARTBEAT.md in your workspace (if it exists).\n\
Follow any instructions or tasks listed there.\n\
If nothing needs attention, reply with just: HEARTBEAT_OK";

/// Token that indicates "nothing to do".
pub const HEARTBEAT_OK_TOKEN: &str = "HEARTBEAT_OK";

/// Check if HEARTBEAT.md has no actionable content.
pub fn is_heartbeat_empty(content: Option<&str>) -> bool {
    match content {
        None => true,
        Some(text) if text.trim().is_empty() => true,
        Some(text) => {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty()
                    || line.starts_with('#')
                    || line.starts_with("<!--")
                    || line == "- [ ]"
                    || line == "* [ ]"
                    || line == "- [x]"
                    || line == "* [x]"
                {
                    continue;
                }
                return false; // Found actionable content
            }
            true
        }
    }
}

/// Heartbeat service configuration.
pub struct HeartbeatConfig {
    pub workspace: PathBuf,
    pub interval_s: u64,
    pub enabled: bool,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            workspace: PathBuf::new(),
            interval_s: DEFAULT_HEARTBEAT_INTERVAL_S,
            enabled: true,
        }
    }
}

/// Get the heartbeat file path.
pub fn heartbeat_file(workspace: &Path) -> PathBuf {
    workspace.join("HEARTBEAT.md")
}

/// Read HEARTBEAT.md content.
pub fn read_heartbeat_file(workspace: &Path) -> Option<String> {
    let path = heartbeat_file(workspace);
    if path.exists() {
        std::fs::read_to_string(&path).ok()
    } else {
        None
    }
}

/// Check if heartbeat should trigger (HEARTBEAT.md has content).
pub fn should_trigger(workspace: &Path) -> bool {
    let content = read_heartbeat_file(workspace);
    !is_heartbeat_empty(content.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_heartbeat_empty() {
        assert!(is_heartbeat_empty(None));
        assert!(is_heartbeat_empty(Some("")));
        assert!(is_heartbeat_empty(Some("# Title\n\n")));
        assert!(is_heartbeat_empty(Some("# Title\n- [ ]\n")));
        assert!(!is_heartbeat_empty(Some("# Tasks\n- Do something")));
    }
}
