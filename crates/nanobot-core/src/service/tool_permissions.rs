//! Tool Permission System
//!
//! Implements three-level permission model for tool execution safety:
//! - AutoApprove: Read-only tools that can execute immediately
//! - RequireConfirmation: Destructive/external tools that need user confirmation
//! - RequireAuth: Admin-only tools (GitHub, system operations)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Three permission levels for tool execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolPermission {
    /// Execute immediately without confirmation (read-only tools)
    ///
    /// Examples: web_search, calculator, datetime, file_read (sandbox only)
    AutoApprove,

    /// Require user confirmation before execution (destructive/external tools)
    ///
    /// Examples: gmail_send, file_write, code_execute, phone_call
    RequireConfirmation,

    /// Require admin authentication (high-risk tools)
    ///
    /// Examples: github_create_or_update_file, github_create_pr, system_shutdown
    RequireAuth,
}

impl ToolPermission {
    /// Check if this permission level requires user interaction
    pub fn requires_approval(&self) -> bool {
        matches!(self, Self::RequireConfirmation | Self::RequireAuth)
    }

    /// Check if this permission level requires admin privileges
    pub fn requires_admin(&self) -> bool {
        matches!(self, Self::RequireAuth)
    }
}

/// Trait for tools with permission levels
pub trait PermissionedTool: Send + Sync {
    /// Get the tool's name
    fn name(&self) -> &str;

    /// Get the permission level for this tool
    fn permission(&self) -> ToolPermission;

    /// Get human-readable confirmation message
    ///
    /// Used when displaying approval request to user.
    fn confirmation_message(&self, args: &HashMap<String, serde_json::Value>) -> String {
        format!(
            "Allow execution of **{}** with these parameters?\n\n```json\n{}\n```",
            self.name(),
            serde_json::to_string_pretty(args).unwrap_or_else(|_| "{}".to_string())
        )
    }
}

/// Approval request sent via SSE to frontend
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolApprovalRequest {
    /// Unique ID for this tool call
    pub tool_call_id: String,

    /// Tool name
    pub tool_name: String,

    /// Tool arguments (for display)
    pub arguments: HashMap<String, serde_json::Value>,

    /// Human-readable confirmation message
    pub message: String,

    /// Permission level required
    pub permission: ToolPermission,
}

impl ToolApprovalRequest {
    /// Create a new approval request
    pub fn new(
        tool_call_id: String,
        tool_name: String,
        arguments: HashMap<String, serde_json::Value>,
        message: String,
        permission: ToolPermission,
    ) -> Self {
        Self {
            tool_call_id,
            tool_name,
            arguments,
            message,
            permission,
        }
    }
}

/// Approval result from user
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalResult {
    /// User approved execution
    Approved,

    /// User denied execution
    Denied,

    /// Approval request timed out (60s default)
    Timeout,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permission_requires_approval() {
        assert!(!ToolPermission::AutoApprove.requires_approval());
        assert!(ToolPermission::RequireConfirmation.requires_approval());
        assert!(ToolPermission::RequireAuth.requires_approval());
    }

    #[test]
    fn test_permission_requires_admin() {
        assert!(!ToolPermission::AutoApprove.requires_admin());
        assert!(!ToolPermission::RequireConfirmation.requires_admin());
        assert!(ToolPermission::RequireAuth.requires_admin());
    }

    #[test]
    fn test_approval_request_serialization() {
        let mut args = HashMap::new();
        args.insert("to".to_string(), serde_json::json!("user@example.com"));
        args.insert("subject".to_string(), serde_json::json!("Test"));

        let req = ToolApprovalRequest::new(
            "tc_123".to_string(),
            "gmail_send".to_string(),
            args,
            "Send email?".to_string(),
            ToolPermission::RequireConfirmation,
        );

        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("tc_123"));
        assert!(json.contains("gmail_send"));
    }
}
