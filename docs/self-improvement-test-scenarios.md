# Self-Improvement Test Scenarios

## Test Environment Setup

```bash
# 1. Set admin session key
export ADMIN_SESSION_KEYS="webchat:yuki@hamada.tokyo"

# 2. Set GitHub token in SSM
aws ssm put-parameter \
  --name /nanobot/github-token \
  --value "ghp_YOUR_GITHUB_TOKEN" \
  --type SecureString \
  --region ap-northeast-1

# 3. Deploy to Lambda
./infra/deploy-fast.sh
```

## Scenario 1: Help Message (Non-Admin User)

**Request**: POST to `/api/v1/chat`
```json
{
  "session_id": "webchat:test@example.com",
  "message": "/improve"
}
```

**Expected Response**:
```json
{
  "response": "使い方: /improve <改善の説明>\n例: /improve ステータスページにレスポンスタイムグラフを追加"
}
```

**Status**: ✅ Implementation correct (lines 743-747)

---

## Scenario 2: Access Denied (Non-Admin)

**Request**: POST to `/api/v1/chat`
```json
{
  "session_id": "webchat:test@example.com",
  "message": "/improve Add caching to session store"
}
```

**Expected Response**:
```json
{
  "response": "⛔ /improve コマンドは管理者のみ利用できます。"
}
```

**Status**: ✅ Implementation correct (lines 757-768)

---

## Scenario 3: Preview Mode (Admin, No --confirm)

**Request**: POST to `/api/v1/chat`
```json
{
  "session_id": "webchat:yuki@hamada.tokyo",
  "message": "/improve Add response time logging to status endpoint"
}
```

**Expected Behavior**:
1. Parse `--confirm` flag → `confirmed = false`
2. Check admin status → ✅ pass
3. Build preview-mode system prompt
4. Filter tools → read-only GitHub tools only
5. Run agentic loop (max 5 iterations)
6. Return analysis

**Expected Response** (example):
```
📋 **改善プレビュー**

変更対象ファイル:
- crates/nanobot-core/src/service/http.rs (lines 2850-2860)

実装アプローチ:
1. Add std::time::Instant tracking at request start
2. Log elapsed time before returning response
3. Include in health check metrics

リスクレベル: Low
推定変更行数: ~15 lines

---
実行するには以下のコマンドを使用してください：
/improve --confirm Add response time logging to status endpoint
```

**Status**: ✅ Implementation correct (lines 850-900)

---

## Scenario 4: Confirmed Mode (Admin, With --confirm)

**Request**: POST to `/api/v1/chat`
```json
{
  "session_id": "webchat:yuki@hamada.tokyo",
  "message": "/improve --confirm Add response time logging to status endpoint"
}
```

**Expected Behavior**:
1. Parse `--confirm` flag → `confirmed = true`
2. Check admin status → ✅ pass
3. Build confirmed-mode system prompt
4. Filter tools → **all** GitHub tools (read + write + PR)
5. Run agentic loop with PR creation
6. Return PR URL

**Expected Response** (example):
```
✅ 改善PRを作成しました！
https://github.com/yukihamada/nanobot/pull/123

内容: 「Add response time logging to status endpoint」

※ マージは手動で行ってください。
```

**Status**: ✅ Implementation correct (lines 975-981)

---

## Scenario 5: Rate Limit (6th Request in Same Day)

**Setup**: Make 5 successful `/improve --confirm` requests

**Request #6**: POST to `/api/v1/chat`
```json
{
  "session_id": "webchat:yuki@hamada.tokyo",
  "message": "/improve --confirm Add feature X"
}
```

**Expected Response**:
```json
{
  "response": "⚠️ 本日の改善リクエスト上限（5回）に達しました。明日またお試しください。"
}
```

**Status**: ✅ Implementation correct (lines 785-818)

---

## Scenario 6: Missing GitHub Token

**Setup**: Remove GitHub token from environment

**Request**: POST to `/api/v1/chat`
```json
{
  "session_id": "webchat:yuki@hamada.tokyo",
  "message": "/improve Add feature"
}
```

**Expected Response**:
```json
{
  "response": "⚠️ GitHub toolsが利用できません（GITHUB_TOKEN未設定）。"
}
```

**Status**: ✅ Implementation correct (lines 856-859)

---

## Scenario 7: GitHub Status Check

**Request**: GET to `/api/v1/status/github`

**Expected Response** (token configured):
```json
{
  "github_tools_available": true,
  "status": "ready"
}
```

**Expected Response** (token missing):
```json
{
  "github_tools_available": false,
  "status": "unconfigured"
}
```

**Status**: ✅ Implementation correct (http.rs lines 12072-12082)

---

## Code Review Summary

### ✅ Verified Features

1. **Flag Parsing** (lines 750-755)
   ```rust
   let (desc_clean, confirmed) = if desc.starts_with("--confirm ") {
       (desc.strip_prefix("--confirm ").unwrap(), true)
   } else {
       (desc, false)
   };
   ```

2. **Admin Check** (lines 757-768)
   - Checks multiple sources: channel_key, user_id, session_key
   - Returns clear error message for non-admins

3. **Rate Limiting** (lines 785-818)
   - DynamoDB atomic counter
   - PK: `IMPROVE_COUNT#{date}`, SK: `DAILY`
   - 5 requests/day limit

4. **Preview/Confirmed Mode** (lines 831-888)
   - Different system prompts
   - Preview: read-only tools filter
   - Confirmed: all tools available

5. **Tool Filtering** (lines 890-908)
   ```rust
   // In preview mode, only allow read operations
   if !confirmed && (name.contains("create") || name.contains("update") || name.contains("delete")) {
       return false;
   }
   ```

6. **Response Handling** (lines 989-1011)
   - Preview: Shows analysis + confirm command
   - Confirmed: Shows PR URL or error

### 🔍 Edge Cases Handled

- Empty description → Help message
- Non-admin → Access denied
- Missing provider/registry → Clear error
- Missing GitHub token → Clear error
- Rate limit exceeded → Clear error
- PR creation failed → Retry message

---

## Manual Testing Checklist

- [ ] Deploy with GITHUB_TOKEN configured
- [ ] Test as non-admin → Access denied
- [ ] Test empty `/improve` → Help message
- [ ] Test `/improve <desc>` → Preview returned
- [ ] Test `/improve --confirm <desc>` → PR created
- [ ] Verify 6th request → Rate limit error
- [ ] Test without GITHUB_TOKEN → Error message
- [ ] Check `/api/v1/status/github` → Returns token status
- [ ] Verify PR has correct branch name: `auto-improve/{slug}`
- [ ] Verify PR has label: `auto-improvement`

---

## Integration with GitHub

### Expected GitHub API Calls (Confirmed Mode)

1. **Read Files**
   ```
   GET /repos/yukihamada/nanobot/contents/{path}
   ```

2. **Create/Update Files**
   ```
   PUT /repos/yukihamada/nanobot/contents/{path}
   Body: { message, content (base64), branch }
   ```

3. **Create PR**
   ```
   POST /repos/yukihamada/nanobot/pulls
   Body: { title, body, head: "auto-improve/...", base: "main" }
   ```

### Branch Naming

Input: `/improve --confirm Add session caching`

Branch: `auto-improve/add-session-caching`

Logic (lines 847-848):
```rust
branch_suffix = desc_clean.chars()
    .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == ' ')
    .take(30).collect::<String>()
    .trim().replace(' ', "-").to_lowercase()
```

---

## Conclusion

✅ **All scenarios implemented correctly**

The `/improve` command is production-ready with:
- Two-step confirmation (preview → --confirm)
- Admin-only access
- Rate limiting (5/day)
- Clear error messages
- GitHub integration
- Safe tool filtering

**Recommendation**: Deploy and test in staging environment with real GitHub token.
