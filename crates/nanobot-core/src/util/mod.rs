pub mod http;
pub mod markdown;

use std::path::{Path, PathBuf};

/// Ensure a directory exists, creating it if necessary.
pub fn ensure_dir(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(path)?;
    Ok(path.to_path_buf())
}

/// Get today's date in YYYY-MM-DD format.
pub fn today_date() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// Get current timestamp in ISO format.
pub fn timestamp() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Convert a string to a safe filename.
pub fn safe_filename(name: &str) -> String {
    const UNSAFE: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
    let mut result = name.to_string();
    for &c in UNSAFE {
        result = result.replace(c, "_");
    }
    result.trim().to_string()
}

/// Parse a session key into (channel, chat_id).
pub fn parse_session_key(key: &str) -> Option<(&str, &str)> {
    key.split_once(':')
}

/// Truncate a string to max length, adding suffix if truncated.
pub fn truncate_string(s: &str, max_len: usize, suffix: &str) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    let mut end = max_len.saturating_sub(suffix.len());
    // Ensure we don't split a multi-byte UTF-8 character
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &s[..end], suffix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_filename() {
        assert_eq!(safe_filename("hello"), "hello");
        assert_eq!(safe_filename("hello world"), "hello world");
        assert_eq!(safe_filename("file<name>"), "file_name_");
        assert_eq!(safe_filename("path/to\\file"), "path_to_file");
        assert_eq!(safe_filename("a:b|c?d*e"), "a_b_c_d_e");
    }

    #[test]
    fn test_parse_session_key() {
        assert_eq!(parse_session_key("telegram:12345"), Some(("telegram", "12345")));
        assert_eq!(parse_session_key("cli:default"), Some(("cli", "default")));
        assert_eq!(parse_session_key("nocolon"), None);
    }

    #[test]
    fn test_truncate_string() {
        assert_eq!(truncate_string("hello", 10, "..."), "hello");
        assert_eq!(truncate_string("hello world", 8, "..."), "hello...");
        assert_eq!(truncate_string("ab", 2, "..."), "ab");
    }

    #[test]
    fn test_today_date_format() {
        let date = today_date();
        assert_eq!(date.len(), 10);
        assert_eq!(date.chars().nth(4), Some('-'));
        assert_eq!(date.chars().nth(7), Some('-'));
    }

    #[test]
    fn test_timestamp_format() {
        let ts = timestamp();
        assert!(ts.contains('T'));
        assert!(ts.len() > 10);
    }

    #[test]
    fn test_ensure_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let subdir = tmp.path().join("a").join("b").join("c");
        assert!(!subdir.exists());
        ensure_dir(&subdir).unwrap();
        assert!(subdir.exists());
    }
}
