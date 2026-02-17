# Tool Permission System

## Overview

nanobot implements a three-level permission model for tool execution to ensure safety and user control.

## Permission Levels

### AutoApprove ✅
**Execute immediately without confirmation**

Read-only tools that cannot modify state or access external services.

Examples:
- `web_search` - Search the web (read-only)
- `calculator` - Mathematical calculations
- `datetime` - Get current date/time
- `file_read` - Read files (sandbox only)

### RequireConfirmation ⚠️
**Require user confirmation before execution**

Destructive or external tools that modify state or interact with services.

Examples:
- `gmail_send` - Send emails
- `file_write` - Write files to disk
- `code_execute` - Execute arbitrary code
- `phone_call` - Make phone calls (Amazon Connect)

### RequireAuth 🔐
**Require admin authentication**

High-risk tools that can modify system configuration or create resources.

Examples:
- `github_create_or_update_file` - Modify repository files
- `github_create_pr` - Create pull requests
- `system_shutdown` - Shutdown system (if implemented)

## Implementation

### Tool Definition

```rust
use crate::service::tool_permissions::{PermissionedTool, ToolPermission};

impl PermissionedTool for MyTool {
    fn name(&self) -> &str {
        "my_tool"
    }

    fn permission(&self) -> ToolPermission {
        ToolPermission::RequireConfirmation
    }

    fn confirmation_message(&self, args: &HashMap<String, serde_json::Value>) -> String {
        let target = args.get("target").and_then(|v| v.as_str()).unwrap_or("unknown");
        format!("Execute my_tool on target: {}?", target)
    }
}
```

### Approval Workflow

1. **Tool call detected** → Check permission level
2. **AutoApprove** → Execute immediately
3. **RequireConfirmation/RequireAuth** → Send SSE approval request
4. **Wait for user response** (60s timeout)
5. **Execute if approved**, skip if denied/timeout

### SSE Event Format

```json
{
  "type": "approval_required",
  "tool_call_id": "tc_abc123",
  "tool_name": "gmail_send",
  "message": "Send email to user@example.com?",
  "permission": "RequireConfirmation",
  "arguments": {
    "to": "user@example.com",
    "subject": "Test"
  }
}
```

### User Response

```http
POST /api/v1/tools/approve
Content-Type: application/json

{
  "tool_call_id": "tc_abc123",
  "approved": true
}
```

## Security Considerations

1. **Default Deny**: New tools should default to `RequireConfirmation`
2. **Timeout**: Approval requests expire after 60 seconds
3. **Admin Check**: `RequireAuth` tools verify admin session keys
4. **Audit Log**: All tool executions logged (especially denied requests)

## Future Enhancements

### Planned Features

1. **Per-User Permissions**
   - Allow users to set default approvals for specific tools
   - "Always allow web_search" preference

2. **MCP Tool Discovery**
   - Automatic risk assessment for new MCP tools
   - LLM-powered capability analysis
   - User-friendly installation prompts

3. **Tool Usage Analytics**
   - Track approval/denial rates
   - Identify frequently blocked tools
   - Optimize permission levels based on usage patterns

4. **Batch Approvals**
   - Approve multiple tool calls at once
   - "Approve all for this session" option

## Troubleshooting

### Tool always requires approval

**Check permission level**:
```rust
let permission = tool_registry.get_permission("tool_name");
println!("Permission: {:?}", permission);
```

If wrong, update the tool's `permission()` implementation.

### Approval timeout

Default timeout is 60 seconds. User must respond within this window.

**Increase timeout** (in http.rs):
```rust
match wait_for_approval(&tc.id, 120).await { // 120s instead of 60s
    // ...
}
```

### Admin-only tools not working

**Check admin session key**:
```rust
let is_admin = std::env::var("ADMIN_SESSION_KEYS")
    .unwrap_or_default()
    .split(',')
    .any(|key| key == session_key);
```

Ensure `ADMIN_SESSION_KEYS` environment variable is set.

## Testing

### Unit Tests

```rust
#[test]
fn test_tool_permission_level() {
    let tool = WebSearchTool::new(None, 5);
    assert_eq!(tool.permission(), ToolPermission::AutoApprove);
}
```

### Integration Tests

```rust
#[tokio::test]
async fn test_approval_workflow() {
    // 1. Tool requires confirmation
    // 2. SSE event sent
    // 3. User approves
    // 4. Tool executes
}
```

## References

- Tool Registry: `src/tool/mod.rs`
- Permission Definitions: `src/service/tool_permissions.rs`
- Approval Workflow: `src/service/http.rs` (agentic loop)
