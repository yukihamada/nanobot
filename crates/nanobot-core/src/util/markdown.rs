use std::collections::HashMap;

/// Parse YAML-like frontmatter from a markdown file.
/// Returns (metadata_map, body_without_frontmatter).
pub fn parse_frontmatter(content: &str) -> (HashMap<String, String>, &str) {
    if !content.starts_with("---") {
        return (HashMap::new(), content);
    }

    // Find the closing ---
    if let Some(end_idx) = content[3..].find("\n---") {
        let frontmatter_str = &content[4..end_idx + 3]; // skip "---\n"
        let body_start = end_idx + 3 + 4; // skip "\n---\n"
        let body = if body_start < content.len() {
            content[body_start..].trim_start_matches('\n')
        } else {
            ""
        };

        let mut metadata = HashMap::new();
        for line in frontmatter_str.lines() {
            if let Some((key, value)) = line.split_once(':') {
                let key = key.trim().to_string();
                let value = value.trim().trim_matches('"').trim_matches('\'').to_string();
                metadata.insert(key, value);
            }
        }

        (metadata, body)
    } else {
        (HashMap::new(), content)
    }
}

/// Strip frontmatter from markdown content.
pub fn strip_frontmatter(content: &str) -> &str {
    let (_, body) = parse_frontmatter(content);
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_frontmatter() {
        let content = "---\ntitle: Test\ndescription: A test skill\nalways: true\n---\n# Body\n\nContent here";
        let (meta, body) = parse_frontmatter(content);
        assert_eq!(meta.get("title").unwrap(), "Test");
        assert_eq!(meta.get("description").unwrap(), "A test skill");
        assert_eq!(meta.get("always").unwrap(), "true");
        assert!(body.starts_with("# Body"));
    }

    #[test]
    fn test_no_frontmatter() {
        let content = "# Just a title\n\nNo frontmatter.";
        let (meta, body) = parse_frontmatter(content);
        assert!(meta.is_empty());
        assert_eq!(body, content);
    }
}
