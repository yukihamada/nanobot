/// Defines the allowed tools for SaaS mode.
///
/// In SaaS mode, dangerous tools (shell, filesystem) are disabled.
/// Only safe tools are available: WebSearch, WebFetch, Memory.

/// List of tool names allowed in SaaS mode.
pub const SAAS_ALLOWED_TOOLS: &[&str] = &[
    "web_search",
    "web_fetch",
    "message",
];

/// List of tool names blocked in SaaS mode.
pub const SAAS_BLOCKED_TOOLS: &[&str] = &[
    "read_file",
    "write_file",
    "edit_file",
    "list_dir",
    "exec",
    "spawn",
];

/// Check if a tool is allowed in SaaS mode.
pub fn is_tool_allowed_in_saas(tool_name: &str) -> bool {
    SAAS_ALLOWED_TOOLS.contains(&tool_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_saas_tool_restrictions() {
        assert!(is_tool_allowed_in_saas("web_search"));
        assert!(is_tool_allowed_in_saas("web_fetch"));
        assert!(is_tool_allowed_in_saas("message"));

        assert!(!is_tool_allowed_in_saas("read_file"));
        assert!(!is_tool_allowed_in_saas("write_file"));
        assert!(!is_tool_allowed_in_saas("exec"));
        assert!(!is_tool_allowed_in_saas("spawn"));
    }
}
