# nanobot Skills & Tools System

> Version: 2.0.0
> Last Updated: 2026-02-18

## Table of Contents
1. [Core System Functions](#1-core-system-functions)
2. [Built-in Tools](#2-built-in-tools)
3. [Integration Tools](#3-integration-tools)
4. [Skill Enhancement Roadmap](#4-skill-enhancement-roadmap)

---

## 1. Core System Functions

### CLI Interaction
- Direct command line interface
- Real-time response system
- Command history and autocomplete
- Error handling and feedback
- Emoji indicators for tool usage (🔧)

### Voice UI
- Speech recognition (STT - Web Speech API)
- Text-to-speech output (TTS - OpenAI tts-1, nova)
- Voice command processing
- Multi-language support (Japanese, English)
- Push-to-talk interaction
- Auto-TTS (automatically reads responses aloud after voice input)

### File Operations
- **read_file** - Read text/binary files, large file support
- **write_file** - Create new files, append mode, permission management
- **edit_file** - In-place editing, backup creation, diff management
- **list_dir** - Recursive listing, filtering, sorting

### Shell Command Execution
- **code_execute** - Sandboxed execution in `/tmp/sandbox/{session_id}/`
- Language support: shell/Python/Node.js
- Security guard patterns (block destructive commands)
- Timeout control (10s)
- Environment variable handling
- Output streaming

### Web Search & Fetch
- **web_search** - Multi-provider (Brave/Bing/Jina) with 3-tier fallback
- **web_fetch** - Jina Reader for JS-heavy pages, HTML parsing
- Rate limiting
- Cache management (15-minute self-cleaning cache)
- Error handling

### Multi-Channel Messaging
- Real-time SSE streaming
- Channel sync (/link command)
- QR code integration
- Deep link support (LINE, Telegram)

### Background Task Management
- Async task execution
- Progress tracking
- Task cancellation
- Notification on completion

### Agentic Mode
- Multi-iteration tool loop (Free=1, Starter=3, Pro=5 iterations)
- Parallel tool execution (up to 5 tools simultaneously)
- SSE progress events (tool_start, tool_result, thinking, content, done)
- Sandbox isolation per session
- Credit deduction per LLM call

---

## 2. Built-in Tools

### 2.1 Core Tools (8)

| Tool | Description | Auto-Approve |
|------|-------------|--------------|
| **web_search** | Search the web using Brave/Bing/Jina APIs | ✅ |
| **web_fetch** | Fetch and parse web content via Jina Reader | ✅ |
| **browser** | Navigate, click, fill forms, take screenshots | ⚠️ |
| **code_execute** | Execute shell/Python/Node.js in sandbox | ⚠️ |
| **calculator** | Evaluate math expressions | ✅ |
| **weather** | Get global weather data | ✅ |
| **wikipedia** | Search Wikipedia articles | ✅ |
| **translation** | Translate text to/from multiple languages | ✅ |

### 2.2 File & Workspace (4)

| Tool | Description | Auto-Approve |
|------|-------------|--------------|
| **file_read** | Read file from sandbox (32KB limit) | ⚠️ |
| **file_write** | Write file to sandbox (100KB limit) | ⚠️ |
| **file_list** | List directory contents (sandbox only) | ✅ |
| **filesystem** | Find files (glob) and search content (grep) | ✅ |

### 2.3 Content Creation (4)

| Tool | Description | Credits |
|------|-------------|---------|
| **image_generate** | Generate images via OpenAI DALL-E | 10 |
| **music_generate** | Generate music via Suno API | 20 |
| **video_generate** | Generate videos via Kling API | 50 |
| **qr_code** | Generate QR codes | 1 |

---

## 3. Integration Tools

### 3.1 Data & Research (4)

| Tool | Description | API Key Required |
|------|-------------|------------------|
| **news_search** | Search news articles | ✅ |
| **youtube_transcript** | Extract YouTube video transcripts | ✅ |
| **arxiv_search** | Search academic papers on arXiv | ❌ |
| **csv_analysis** | Analyze CSV data | ❌ |

### 3.2 Productivity (7)

| Tool | Description | API Key Required |
|------|-------------|------------------|
| **google_calendar** | Read/write calendar events | ✅ OAuth |
| **gmail** | Send/read emails | ✅ OAuth |
| **slack** | Send Slack messages | ✅ Webhook |
| **discord** | Send Discord messages | ✅ Webhook |
| **notion** | Read/write Notion pages | ✅ Integration Token |
| **postgresql** | Query PostgreSQL databases | ✅ Connection String |
| **spotify** | Control Spotify playback | ✅ OAuth |

### 3.3 Development (4)

| Tool | Description | API Key Required |
|------|-------------|------------------|
| **github** | Read/write files, create PRs | ✅ Personal Access Token |
| **webhook** | Send HTTP webhooks | ❌ |
| **phone_call** | Make phone calls (Twilio) | ✅ Twilio Credentials |
| **web_deploy** | Deploy static sites | ✅ Deployment Token |

---

## 4. Skill Enhancement Roadmap

### 4.1 Plugin System

#### Objectives
- Simplify third-party tool integration
- Enable community contributions
- Support hot-reload without redeployment

#### Implementation Plan

##### Phase 1: Plugin Architecture (Q2 2026)
- **WASM-based plugins** for sandboxed execution
- **Trait-based API**: `Plugin` trait with `execute()`, `metadata()`, `permissions()`
- **Dynamic loading**: `libloading` crate for DLL/SO loading
- **Version management**: Semantic versioning, compatibility checks

```rust
pub trait Plugin: Send + Sync {
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    fn execute(&self, args: ToolArgs) -> ToolResult;
    fn permissions(&self) -> ToolPermission;
}
```

##### Phase 2: Plugin Registry (Q3 2026)
- **Centralized registry** at https://plugins.chatweb.ai
- **Manifest format**: JSON with dependencies, permissions, metadata
- **CLI commands**: `nanobot plugin install <name>`, `nanobot plugin list`

##### Phase 3: Security (Q3 2026)
- **Sandboxing**: Capability-based security (only allowed APIs)
- **Code signing**: GPG signatures for verified publishers
- **Permission review**: User approval for high-risk permissions

### 4.2 Skill Marketplace

#### Objectives
- Foster community ecosystem
- Monetize premium skills
- Provide discoverable catalog

#### Features

##### Discovery
- **Search & filter**: by category, rating, popularity, price
- **Detailed pages**: README, changelog, dependencies, reviews
- **Try before buy**: Free trial period (7 days)

##### Developer Program
- **Revenue share**: 70% to developer, 30% to platform
- **Payout**: Monthly via Stripe Connect
- **Analytics**: Downloads, revenue, user feedback

##### Quality Control
- **Review process**: Manual review for initial approval
- **Automated tests**: Security scans, performance benchmarks
- **User ratings**: 5-star system, verified reviews only

### 4.3 Memory System Optimization

#### Current State
- DynamoDB direct access
- Client-side cosine similarity calculation
- Manual embedding generation

#### Improvements

##### Redis Cache Layer (Q2 2026)
- **Session cache**: 1-hour TTL for active sessions
- **LRU eviction**: Automatically remove least-recently-used entries
- **Write-through**: Update both Redis and DynamoDB

##### Vector Database Integration (Q3 2026)
- **Options**: Pinecone, Qdrant, Weaviate
- **Benefits**: Sub-10ms search latency, HNSW indexing
- **Migration**: DynamoDB Streams trigger for automatic embedding

##### Automatic Summarization (Q4 2026)
- **Context window management**: Summarize conversations > 50 messages
- **Importance scoring**: Prioritize high-value memories
- **Archive policy**: Move 90-day-old conversations to cold storage (S3)

### 4.4 Security Enhancements

#### Audit System (Q2 2026)
- **Comprehensive logging**: All API calls, tool executions, credit transactions
- **Anomaly detection**: ML-based detection of unusual patterns
- **Compliance reports**: GDPR, CCPA, SOC 2 compliance exports

#### Role-Based Access Control (Q3 2026)
- **Roles**: Admin, Developer, User, Guest
- **Granular permissions**: Tool-level allow/deny
- **OAuth2/OIDC**: Integration with Google, GitHub, Microsoft

#### End-to-End Encryption (Q4 2026)
- **E2EE mode**: Optional for privacy-sensitive conversations
- **Key management**: AWS KMS integration, user-controlled keys
- **Forward secrecy**: Rotate keys every 7 days

### 4.5 Monitoring & Observability

#### Detailed Metrics (Q2 2026)
- **Tool execution time**: P50/P90/P99 latency per tool
- **Provider success rates**: Track failover patterns
- **Channel activity**: Active users per channel per day
- **Credit consumption trends**: Predict when users will upgrade

#### Alerting (Q2 2026)
- **Error rate**: Alert if > 5% for 5 minutes
- **Latency**: Alert if P99 > 500ms for 10 minutes
- **Credit balance**: Notify user when < 100 credits remain
- **Notification channels**: Slack, PagerDuty, Email

#### Distributed Tracing (Q3 2026)
- **AWS X-Ray**: End-to-end request tracing
- **Honeycomb integration**: Detailed query analysis
- **Dependency maps**: Visualize service relationships
- **Performance profiling**: Identify bottlenecks

---

## 5. Command System

### Slash Commands

All channels support slash commands for special operations:

| Command | Description | Auth Required |
|---------|-------------|---------------|
| **/help** | Show command list | ❌ |
| **/status** | Show LLM provider status (latency ms) | ❌ |
| **/share** | Generate conversation share URL (`/c/{hash}`) | ✅ |
| **/link [CODE]** | Link channel to account | ❌ |
| **/improve <description>** | Create self-improvement PR | ✅ Admin |

### Self-Improvement (/improve)

**Purpose**: Allow nanobot to propose and implement code improvements

**Workflow**:
1. User: `/improve Add support for Notion API`
2. nanobot analyzes codebase, proposes changes
3. User reviews and approves
4. nanobot creates GitHub PR with implementation
5. CI/CD runs tests, deploys if passing

**Requirements**:
- GitHub Personal Access Token with `repo` scope
- Admin session key
- Approval workflow (can't auto-merge to main)

---

## 6. References

- [SYSTEM.md](SYSTEM.md) - Complete system architecture (Japanese)
- [README.md](../README.md) - Project overview
- [deployment.md](deployment.md) - Deployment guide
- [tool-permissions.md](tool-permissions.md) - Tool permission system
- [CLAUDE.md](../CLAUDE.md) - Developer guide

---

**This document describes nanobot v2.0.0 skills and tools. For implementation details, see module documentation.**
