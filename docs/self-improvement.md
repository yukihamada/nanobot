# Self-Improvement System

## Overview

nanobot can improve its own code based on user feedback and admin requests. This document describes the architecture, safety mechanisms, and usage of the self-improvement system.

## Architecture

### Components

1. **Feedback Collection**
   - Automatic collection via Web UI (thumbs up/down buttons)
   - Stored in DynamoDB with timestamps
   - Negative feedback from last 7 days included in improvement context

2. **Command Interface**
   - `/improve <description>` — Preview mode (default)
   - `/improve --confirm <description>` — Execute mode
   - Admin-only access (requires ADMIN_SESSION_KEYS)
   - Daily rate limit: 5 PRs/day

3. **Agentic Loop**
   - Powered by Claude Sonnet 4.5
   - Max 5 iterations for analysis and PR creation
   - GitHub tools: read_file, create_or_update_file, create_pr
   - Automatic branch naming: `auto-improve/{description-slug}`

4. **Pull Request Creation**
   - Creates feature branch (never commits to main/master)
   - Adds label: `auto-improvement`
   - Includes context: user feedback + admin request
   - Returns PR URL for human review

### Data Flow

```
User Feedback (DynamoDB)
         ↓
Admin: /improve <desc>
         ↓
Preview Analysis
    (read-only tools)
         ↓
Admin: /improve --confirm <desc>
         ↓
Rate Limit Check (5/day)
         ↓
Agentic Loop (Claude Sonnet 4.5)
    ↓              ↓
Read Files    Modify Files
         ↓
Create PR on Feature Branch
         ↓
Human Review & Merge
```

## Safety Mechanisms

### 1. Admin-Only Access

Only users with admin session keys can execute `/improve`:

```bash
# Environment variable (Lambda)
ADMIN_SESSION_KEYS=webchat:yuki@hamada.tokyo,webchat:mail@yukihamada.jp
```

Admin check hierarchy:
1. Channel key (e.g., LINE user ID)
2. User ID (registered email)
3. Session key (webchat)

### 2. Daily Rate Limiting

Maximum 5 improvement requests per day (UTC).

**DynamoDB Schema**:
```
PK: IMPROVE_COUNT#{YYYY-MM-DD}
SK: DAILY
Attributes:
  - count: number (atomic counter)
  - ttl: 2 days after creation
```

If limit reached, returns: "⚠️ 本日の改善リクエスト上限（5回）に達しました。"

### 3. Two-Step Confirmation

**Preview Mode (default)**:
- Uses **read-only** GitHub tools (`github_read_file`)
- Provides:
  - List of files to be changed
  - High-level implementation approach
  - Risk level (Low/Medium/High)
  - Estimated complexity
- No commits, no PRs

**Confirmed Mode (`--confirm` flag)**:
- Uses **all** GitHub tools (read + write + PR creation)
- Creates feature branch
- Commits changes
- Opens PR

**Why?**
Prevents accidental PR creation. Admin reviews preview first.

### 4. Feature Branch Only

PRs are **never** created on `main` or `master` branches.

Branch naming convention:
```
auto-improve/{description-slug}
```

Example: `auto-improve/fix-timeout-messages`

### 5. Human Review Required

- All PRs require manual merge (no auto-merge)
- GitHub Actions CI must pass
- At least 1 approving review recommended

### 6. GITHUB_TOKEN Validation

Before deployment, `deploy-fast.sh` checks:

```bash
aws ssm get-parameter --name /nanobot/github-token --region ap-northeast-1
```

If missing, deployment continues with warning (GitHub tools disabled).

## Usage

### Prerequisites

1. **GITHUB_TOKEN** in SSM Parameter Store:
   ```bash
   aws ssm put-parameter \
     --name /nanobot/github-token \
     --value "ghp_YOUR_TOKEN_HERE" \
     --type SecureString \
     --region ap-northeast-1
   ```

2. **Admin session key** in Lambda environment variables:
   ```bash
   ADMIN_SESSION_KEYS=webchat:yuki@hamada.tokyo
   ```

### Step 1: Preview Changes

```
/improve Optimize session caching to reduce DynamoDB reads
```

**Response**:
```
📋 改善プレビュー

変更対象ファイル:
- crates/nanobot-core/src/service/http.rs (lines 3800-3850)
- crates/nanobot-core/src/session/store.rs (add caching layer)

実装アプローチ:
1. Add in-memory LRU cache (DashMap)
2. Cache sessions for 5 minutes
3. Invalidate on write operations

リスクレベル: Low
推定変更行数: ~80 lines

---
実行するには以下のコマンドを使用してください：
/improve --confirm Optimize session caching to reduce DynamoDB reads
```

### Step 2: Execute (after reviewing preview)

```
/improve --confirm Optimize session caching to reduce DynamoDB reads
```

**Response**:
```
✅ 改善PRを作成しました！
https://github.com/yukihamada/nanobot/pull/123

内容: 「Optimize session caching to reduce DynamoDB reads」

※ マージは手動で行ってください。
```

### Feedback Integration

User feedback automatically influences PR prioritization:

1. **User clicks 👎 (negative feedback)**
   ```
   DynamoDB: FEEDBACK#{user_id}#{timestamp}
   Attributes:
     - rating: "down"
     - message: "Response was too slow"
     - context: (last 5 messages)
   ```

2. **Admin runs `/improve`**
   - System queries feedback from last 7 days
   - Includes top 5 negative feedback items in context
   - LLM considers these pain points when planning improvements

3. **Result**
   - PRs address real user issues
   - Data-driven improvements

## API Endpoints

### GitHub Status Check

**GET** `/api/v1/status/github`

Response:
```json
{
  "github_tools_available": true,
  "status": "ready"
}
```

Used by:
- Deployment validation (`deploy-fast.sh`)
- Admin dashboard
- Self-improvement command pre-flight check

## Error Handling

### Missing GITHUB_TOKEN

```
⚠️ GitHub toolsが利用できません（GITHUB_TOKEN未設定）。
```

**Fix**:
```bash
aws ssm put-parameter --name /nanobot/github-token --value "ghp_..." --type SecureString
```

### Rate Limit Exceeded

```
⚠️ 本日の改善リクエスト上限（5回）に達しました。明日またお試しください。
```

**Reset**: Automatic at 00:00 UTC (DynamoDB TTL)

### LLM Error

```
⚠️ LLMエラーが発生しました: <error details>
```

**Retry**: Wait 1 minute, try again (potential rate limit)

## Future Enhancements

### Planned Features

1. **Auto-merge for low-risk PRs** (opt-in)
   - Requires all CI checks passing
   - Only for Low risk level
   - Admin approval in config

2. **Feedback-triggered improvements** (autonomous)
   - If 10+ users report same issue → auto-create preview
   - Admin receives notification with preview link

3. **A/B testing integration**
   - Deploy PR to canary Lambda alias
   - Compare metrics (latency, credits, feedback)
   - Auto-promote if metrics improve

4. **Multi-repo support**
   - Support for elio, elio-api, openclaw
   - Cross-repo coordinated changes

## Troubleshooting

### PR not created

**Symptoms**: "🔧 改善処理を実行しましたが、PRの作成には至りませんでした。"

**Possible causes**:
1. LLM reached max iterations without completing task
2. GitHub API error (check CloudWatch logs)
3. Permission issue (token lacks `repo` scope)

**Solution**:
- Simplify request description
- Check token permissions: `repo`, `workflow`
- Retry with more specific instructions

### Preview shows no changes

**Symptoms**: Analysis is empty or generic

**Possible causes**:
1. Request too vague
2. LLM couldn't find relevant files

**Solution**:
- Be more specific: mention file names, features
- Example: "Add retry logic to `http.rs` line 3000"

### Branch already exists

**Symptoms**: PR creation fails with "branch exists"

**Solution**:
- Delete old branch manually on GitHub
- Or use different description (generates new branch name)

## Metrics

Track self-improvement effectiveness:

```sql
-- DynamoDB query (pseudo-SQL)
SELECT COUNT(*) as pr_count, AVG(lines_changed) as avg_lines
FROM IMPROVE_COUNT
WHERE created_at > NOW() - INTERVAL 30 DAYS
```

Monitor:
- PRs created per week
- Merge rate (successful / total)
- Time to merge
- User feedback improvement (before/after)

## Security Considerations

1. **Token Security**
   - GITHUB_TOKEN stored in SSM Parameter Store (encrypted)
   - Lambda role has `ssm:GetParameter` permission
   - Token has minimum required scopes (`repo`, `workflow`)

2. **Code Injection Prevention**
   - LLM cannot execute arbitrary code on Lambda
   - All file modifications reviewed before merge
   - GitHub Actions CI validates changes

3. **Rate Limiting**
   - Prevents abuse (5 PRs/day limit)
   - Admin-only access

4. **Audit Trail**
   - All `/improve` requests logged to CloudWatch
   - DynamoDB stores request history (90-day TTL)

## References

- GitHub API: https://docs.github.com/en/rest
- Claude API: https://docs.anthropic.com/claude/reference
- OpenClaw (inspiration): https://github.com/hamada-infocube/openclaw
