<div align="center">

# nanobot

[![Build](https://github.com/yukihamada/nanobot/actions/workflows/ci.yml/badge.svg)](https://github.com/yukihamada/nanobot/actions)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![GitHub stars](https://img.shields.io/github/stars/yukihamada/nanobot?style=social)](https://github.com/yukihamada/nanobot/stargazers)
[![Rust](https://img.shields.io/badge/Built_with-Rust-dea584?logo=rust)](https://www.rust-lang.org/)

**Voice-First AI Agent Platform** — Deploy intelligent AI agents to LINE, Telegram, Web in minutes.

[Try it now](https://chatweb.ai) | [API Docs](https://chatweb.ai/docs) | [Status](https://chatweb.ai/status) | [Playground](https://chatweb.ai/playground)

</div>

---

## Why nanobot?

Most AI agent frameworks are Python wrappers around a single model, or developer-only tools limited to the terminal. nanobot is different: a **production-grade Rust runtime** that connects any LLM to any channel, with voice, tools, and long-term memory built in.

### Comparison with Other AI Agent Frameworks

| | **ChatWeb (nanobot)** | **OpenClaw** | **Claude Code** | **AutoGPT** | **OpenHands** |
|---|---|---|---|---|---|
| **Language** | Rust (axum) | TypeScript | CLI (proprietary) | Python/TS | Python/TS |
| **Deployment** | AWS Lambda (serverless) | Local/VPS | Local/SaaS | Self-hosted | Local/Cloud/K8s |
| **Cold Start** | <50ms | ~1s (Node.js) | N/A | ~3-10s | ~3-10s |
| **Channels** | **14+** (Web, LINE, Telegram, WhatsApp, Discord, Slack, Teams, Facebook, Zalo, Feishu, Google Chat...) | 11+ (WhatsApp, Telegram, Slack, Discord, Signal, Teams...) | 1 (Terminal/IDE) | 1 (Web UI) | 4 (Web, CLI, Slack, Jira) |
| **Voice (STT+TTS)** | Yes (push-to-talk, auto-TTS) | Partial (Whisper + ElevenLabs) | No | No | No |
| **Models** | Claude, GPT-4o, Gemini, Groq, DeepSeek, Qwen, Kimi (hot-swap) | Claude, GPT (via API keys) | Claude only | GPT-4o, Claude | Any (configurable) |
| **Auto Failover** | Yes (primary → gpt-4o-mini → gemini-flash) | No | No | No | No |
| **Memory** | 2-layer (daily log + long-term, auto-consolidation) | Session + transcript | Conversation only | Workspace storage | Project-based |
| **Tool Count** | 16+ built-in + sandboxed code exec | 50+ (ClawHub ecosystem) | fs, git, shell, MCP | Marketplace agents | Developer tools |
| **Agentic Loop** | 1-5 iterations (plan-based) | Continuous | 7 parallel agents | Continuous | Iterative |
| **Pricing** | Credit-based (100 free, from $9/mo) | Free (BYOK) | $17-$100+/mo | Free (self-hosted) | Free ($10 cloud) |
| **License** | MIT | MIT | Proprietary | Polyform + MIT | MIT |
| **Best For** | Voice-first multi-channel AI | Privacy-first personal assistant | Developer workflows | Automation | Software engineering |

### Key Differentiators

- **Voice-First**: The only framework with native STT + TTS + push-to-talk UI
- **Most Channels**: 14+ channels — LINE, Telegram, WhatsApp, Discord, Slack, Teams, Facebook, Zalo, Feishu, Google Chat, and more. Cross-channel conversation sync via `/link`
- **Fastest Runtime**: Rust on Lambda ARM64 = sub-50ms cold start, <2s response
- **Auto Failover**: Primary model fails → automatically retries with cheaper models
- **Long-Term Memory**: Daily conversation logs auto-consolidated into long-term memory (DynamoDB)
- **Serverless Scale**: Zero-to-infinite scale on AWS Lambda, no VPS to manage

---

## Features

### Core
- **Voice-First** — Push-to-talk microphone UI with speech-to-text and auto-TTS response
- **Multi-Model** — Claude, GPT-4o, Gemini Flash, Groq, DeepSeek, Qwen, Kimi. Automatic model selection per channel + LLM failover
- **Multi-Channel** — Web, LINE, Telegram, Facebook, WhatsApp, Discord, Slack, Teams, Zalo, Feishu, Google Chat. One conversation synced across all channels
- **Auto Failover** — Primary model fails? Automatically retries with gpt-4o-mini → gemini-flash. No error shown to user
- **Long-Term Memory** — 2-layer memory: daily conversation logs + long-term facts. Auto-consolidated every 10 entries via cheap LLM. Yesterday's context included for continuity

### Tools & Integrations
- **Web Search** — Real-time web search via Brave/Google
- **Code Execution** — Sandboxed shell/Python/Node.js execution (per-session `/tmp/sandbox/`)
- **File Operations** — Read, write, list files in sandbox (with path traversal protection)
- **Weather** — Live weather data for any location
- **Calculator** — Mathematical expression evaluation
- **Web Fetch** — Extract content from any URL
- **Google Calendar** — View and create events (OAuth linked)
- **Gmail** — Search, read, send emails (OAuth linked)
- **Wikipedia** — Quick encyclopedia lookup
- **Translation** — Multi-language translation
- **QR Code** — Generate QR codes
- **News Search** — Latest news aggregation

### Developer Platform
- **REST API** — Full-featured JSON API for chat, speech, sessions, and more
- **SSE Streaming** — Real-time streaming responses via Server-Sent Events
- **MCP Server** — Model Context Protocol endpoint for AI agent tool use
- **API Playground** — Interactive API explorer with shareable results
- **API Keys** — Create and manage API keys for programmatic access
- **Slash Commands** — `/link`, `/share`, `/help`, `/status`, `/improve`
- **Settings API** — Model, temperature, max_tokens, BYOK API keys per user
- **Memory API** — Read and clear long-term memory via REST

### Infrastructure
- **Serverless Rust** — Compiled to ARM64, runs on AWS Lambda with sub-50ms cold starts
- **DynamoDB** — Single-table design for sessions, users, memory, billing, and more
- **AI Agent Friendly** — `/robots.txt`, `/llms.txt`, `/.well-known/ai-plugin.json`
- **Context Summarization** — Long conversations auto-summarized instead of silently truncated

---

## Quick Start

### Chat via API (no auth required)

```bash
curl -X POST https://chatweb.ai/api/v1/chat \
  -H "Content-Type: application/json" \
  -d '{"message": "京都の旅行プランを立てて", "session_id": "my-session"}'
```

### Stream responses (SSE)

```bash
curl -N https://chatweb.ai/api/v1/chat/stream \
  -H "Content-Type: application/json" \
  -d '{"message": "Write me a haiku about Rust", "session_id": "my-session"}'
```

### Text-to-Speech

```bash
curl -X POST https://chatweb.ai/api/v1/speech/synthesize \
  -H "Content-Type: application/json" \
  -d '{"text": "こんにちは！", "voice": "nova"}' \
  --output speech.mp3
```

### Try it on every channel

| Channel | Link |
|---------|------|
| Web | [chatweb.ai](https://chatweb.ai) |
| LINE | [@619jcqqh](https://line.me/R/ti/p/@619jcqqh) |
| Telegram | [@chatweb_ai_bot](https://t.me/chatweb_ai_bot) |

---

## Architecture

```
 Web  LINE  Telegram  Facebook
  |     |      |         |
  +-----+------+---------+
         |
   API Gateway (chatweb.ai)
         |
   AWS Lambda (Rust, ARM64)
         |
   +-----+-----+-----+
   |           |           |
DynamoDB    LLM APIs    Tools
sessions    Anthropic   web_search
users       OpenAI      calculator
memory      Gemini      web_fetch
billing     Groq/Kimi   weather
```

### Provider Fallback Strategy

```
Request → Primary (25s timeout)
            ├── Success → Return
            └── Fail/Timeout
                  ↓
          All Remaining Providers (parallel, 25s each)
            ├── First Success → Return
            └── All Fail → Error
```

---

## API Reference

### Chat

| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| `POST` | `/api/v1/chat` | Optional | Send a message, get an AI response |
| `POST` | `/api/v1/chat/stream` | Optional | SSE streaming response |
| `POST` | `/api/v1/chat/explore` | Optional | Parallel multi-model comparison |

### Authentication

| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| `POST` | `/api/v1/auth/register` | - | Create account (email + password) |
| `POST` | `/api/v1/auth/login` | - | Login, get bearer token |
| `POST` | `/api/v1/auth/email` | - | Send email verification code |
| `POST` | `/api/v1/auth/verify` | - | Verify email code |
| `GET` | `/api/v1/auth/me` | Bearer | Current user + credits |
| `GET` | `/auth/google` | - | Google OAuth flow |

### Conversations

| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| `GET` | `/api/v1/conversations` | Bearer | List conversations |
| `POST` | `/api/v1/conversations` | Bearer | Create conversation |
| `GET` | `/api/v1/conversations/{id}/messages` | Bearer | Get messages |
| `DELETE` | `/api/v1/conversations/{id}` | Bearer | Delete conversation |
| `POST` | `/api/v1/conversations/{id}/share` | Bearer | Generate share link |
| `DELETE` | `/api/v1/conversations/{id}/share` | Bearer | Revoke share link |
| `GET` | `/api/v1/shared/{hash}` | - | Read shared conversation |

### Sessions & Accounts

| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| `GET` | `/api/v1/sessions` | x-session-id | List sessions |
| `GET` | `/api/v1/sessions/{id}` | x-session-id | Get session with history |
| `DELETE` | `/api/v1/sessions/{id}` | x-session-id | Delete a session |
| `GET` | `/api/v1/account/{id}` | - | User profile, plan, credits |
| `GET` | `/api/v1/usage` | x-session-id | Credit usage stats |

### Speech

| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| `POST` | `/api/v1/speech/synthesize` | Optional | Text-to-speech (MP3, OpenAI TTS) |

### API Keys

| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| `GET` | `/api/v1/apikeys` | Bearer | List API keys |
| `POST` | `/api/v1/apikeys` | Bearer | Create API key |
| `DELETE` | `/api/v1/apikeys/{id}` | Bearer | Delete API key |

### Channel Linking

| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| `POST` | `/api/v1/link/generate` | x-session-id | Generate 6-char link code for QR flow |

### Billing

| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| `POST` | `/api/v1/billing/checkout` | Bearer | Create Stripe checkout session |
| `GET` | `/api/v1/billing/portal` | Bearer | Stripe customer portal URL |
| `POST` | `/api/v1/coupon/validate` | - | Validate coupon code |
| `POST` | `/api/v1/coupon/redeem` | Bearer | Redeem coupon code |

### Sync (Cross-device)

| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| `GET` | `/api/v1/sync/conversations` | Bearer | List sync-enabled conversations |
| `GET` | `/api/v1/sync/conversations/{id}` | Bearer | Get synced conversation |
| `POST` | `/api/v1/sync/push` | Bearer | Push conversation update |

### Devices

| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| `GET` | `/api/v1/devices` | x-session-id | List registered devices |
| `POST` | `/api/v1/devices/heartbeat` | x-session-id | Device heartbeat |

### System

| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| `GET` | `/api/v1/providers` | - | Available AI providers |
| `GET` | `/api/v1/integrations` | - | Available tools |
| `GET` | `/api/v1/agents` | - | Available agent types |
| `GET` | `/api/v1/settings/{id}` | - | User settings |
| `POST` | `/api/v1/settings/{id}` | - | Update settings |
| `GET` | `/api/v1/status/ping` | - | Provider health check |
| `GET` | `/health` | - | Service health |
| `POST` | `/mcp` | - | MCP JSON-RPC 2.0 endpoint |

### Webhooks

| Method | Endpoint | Description |
|--------|----------|-------------|
| `POST` | `/webhooks/line` | LINE Messaging API webhook |
| `POST` | `/webhooks/telegram` | Telegram Bot API webhook |
| `GET/POST` | `/webhooks/facebook` | Facebook Messenger webhook |
| `POST` | `/webhooks/stripe` | Stripe payment events |

### AI Agent Discovery

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/robots.txt` | Crawler directives |
| `GET` | `/llms.txt` | LLM-friendly API summary |
| `GET` | `/llms-full.txt` | Detailed API spec for LLMs |
| `GET` | `/.well-known/ai-plugin.json` | OpenAI plugin manifest |

### Pages

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/` | Main web application (SPA) |
| `GET` | `/pricing` | Pricing page |
| `GET` | `/docs` | API documentation |
| `GET` | `/status` | Service status dashboard |
| `GET` | `/playground` | Interactive API playground |
| `GET` | `/contact` | Contact form |
| `GET` | `/comparison` | AI model comparison |
| `GET` | `/welcome` | Onboarding page |
| `GET` | `/admin` | Admin dashboard |
| `GET` | `/c/{hash}` | Shared conversation view |

---

## Slash Commands

| Command | Description | Access |
|---------|-------------|--------|
| `/link` | Generate a 6-digit channel linking code | Everyone |
| `/link CODE` | Link current channel to another | Everyone |
| `/share` | Generate a shareable conversation link | Authenticated |
| `/help` | Show available commands | Everyone |
| `/status` | Show provider status inline | Everyone |
| `/improve <desc>` | Request a self-improvement PR | Admin only |

---

## MCP Server

nanobot exposes an [MCP (Model Context Protocol)](https://modelcontextprotocol.io/) endpoint at `POST /mcp` for AI agent integration.

**Available tools:**
- `chatweb_chat` — Send a message and get an AI response
- `chatweb_tts` — Convert text to speech (MP3)
- `chatweb_providers` — List available AI providers
- `chatweb_status` — Check service health

```json
// Example: Initialize MCP session
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}

// Example: Call chat tool
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"chatweb_chat","arguments":{"message":"Hello!"}}}
```

---

## Channel Sync

Conversations stay in sync across all channels:

1. **QR Code** — Click LINE/Telegram button on web → scan QR → send pre-filled message → linked
2. **Link Code** — Send `/link` in any channel → get 6-digit code → send `/link CODE` in another channel
3. **Session ID** — Copy your `webchat:xxxx-...` ID from the web UI and send it in LINE/Telegram

---

## Self-Hosting

### Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `ANTHROPIC_API_KEY` | At least one LLM key | Anthropic API key |
| `OPENAI_API_KEY` | At least one LLM key | OpenAI API key |
| `GEMINI_API_KEY` | Optional | Google Gemini API key |
| `GROQ_API_KEY` | Optional | Groq inference API key |
| `KIMI_API_KEY` | Optional | Kimi/Moonshot API key |
| `OPENROUTER_API_KEY` | Optional | OpenRouter (multi-model fallback) |
| `DYNAMODB_TABLE` | For Lambda | DynamoDB table name |
| `BASE_URL` | Optional | Custom base URL (default: `https://chatweb.ai`) |
| `LINE_CHANNEL_SECRET` | For LINE | LINE channel secret |
| `LINE_CHANNEL_ACCESS_TOKEN` | For LINE | LINE channel access token |
| `TELEGRAM_BOT_TOKEN` | For Telegram | Telegram bot token |
| `FACEBOOK_PAGE_ACCESS_TOKEN` | For Facebook | Facebook page access token |
| `FACEBOOK_VERIFY_TOKEN` | For Facebook | Facebook webhook verify token |
| `STRIPE_SECRET_KEY` | For billing | Stripe secret key |
| `GOOGLE_CLIENT_ID` | For OAuth | Google OAuth client ID |
| `GOOGLE_CLIENT_SECRET` | For OAuth | Google OAuth client secret |
| `PASSWORD_HMAC_KEY` | Recommended | HMAC key for password hashing |
| `ADMIN_SESSION_KEYS` | Optional | Comma-separated admin session keys |

### Docker

```bash
docker run -p 3000:3000 \
  -e ANTHROPIC_API_KEY=sk-... \
  -e OPENAI_API_KEY=sk-... \
  ghcr.io/yukihamada/nanobot
```

### AWS Lambda (ARM64)

```bash
brew install zig && cargo install cargo-zigbuild
rustup target add aarch64-unknown-linux-gnu

RUSTUP_TOOLCHAIN=stable \
RUSTC=~/.rustup/toolchains/stable-aarch64-apple-darwin/bin/rustc \
cargo zigbuild \
  --manifest-path crates/nanobot-lambda/Cargo.toml \
  --release --target aarch64-unknown-linux-gnu

cp target/aarch64-unknown-linux-gnu/release/bootstrap ./bootstrap
zip -j lambda.zip bootstrap
aws lambda update-function-code --function-name nanobot --zip-file fileb://lambda.zip
```

### Local Development

```bash
cargo run -- gateway --http --http-port 3000
# Open http://localhost:3000
```

---

## Project Structure

```
crates/
  nanobot-core/src/
    service/
      http.rs           Main HTTP handler (70+ routes)
      commands.rs        Slash command framework (/link, /share, /help, /status, /improve)
      auth.rs            Authentication & credit calculation
      integrations.rs    Tool integrations (web_search, calculator, etc.)
    provider/
      mod.rs             LoadBalancedProvider with parallel fallback
      anthropic.rs       Anthropic Claude provider
      openai_compat.rs   OpenAI-compatible provider (GPT, Groq, Kimi, OpenRouter)
      gemini.rs          Google Gemini provider
    channel/
      line.rs            LINE Messaging API
      telegram.rs        Telegram Bot API
      facebook.rs        Facebook Messenger
    memory/              Long-term memory (DynamoDB / file-based)
    session/             Session management
  nanobot-lambda/        AWS Lambda handler wrapper
infra/
  template.yaml          SAM template
  deploy.sh              Deploy script
web/
  index.html             Main SPA (voice-first chat UI)
  docs.html              API documentation
  status.html            Service status dashboard
  playground.html        Interactive API playground
  pricing.html           Pricing page with coupon UI
src/
  main.rs                Local server CLI
tests/                   Integration tests
```

---

## DynamoDB Schema

Single-table design with composite keys (`pk` + `sk`):

| PK Pattern | SK | Purpose |
|------------|-----|---------|
| `USER#{id}` | `PROFILE` | User profile and credits |
| `AUTH#{token}` | `SESSION` | Auth session tokens |
| `USAGE#{id}#{date}` | `DAILY` | Daily usage tracking |
| `MEMORY#{id}` | `LONG_TERM` / `DAILY#{date}` | Long-term memory |
| `CONV#{user}#{id}` | `META` / `MSG#{ts}` | Conversations and messages |
| `LINK#{channel_key}` | `CHANNEL_MAP` | Channel linking map |
| `LINKCODE#{code}` | `PENDING` | Pending link codes (30min TTL) |
| `SHARE#{hash}` | `INFO` | Shared conversation links |
| `RESULT#{id}` | `DATA` | Playground shared results (30d TTL) |
| `AUDIT#{date}` | `{timestamp}` | Audit log (90d TTL) |
| `APIKEY#{user}#{id}` | `KEY` | API keys |

---

## Pricing

| Plan | Price | Credits | Models |
|------|-------|---------|--------|
| **Free** | $0/mo | 1,000 | GPT-4o-mini, Gemini Flash |
| **Starter** | $9/mo | 25,000 | + GPT-4o, Claude Sonnet |
| **Pro** | $29/mo | 300,000 | + Claude Opus, all models |

---

## Ecosystem

| Product | Description | Link |
|---------|-------------|------|
| **chatweb.ai** | Consumer AI assistant (Web, LINE, Telegram) | [chatweb.ai](https://chatweb.ai) |
| **teai.io** | Developer platform and API | [teai.io](https://teai.io) |
| **ElioChat** | Offline-capable iOS AI companion | [App Store](https://apps.apple.com/app/eliochat/id6742071881) |

---

## Contributing

```bash
git clone https://github.com/yukihamada/nanobot.git
cd nanobot
cargo test --all
cargo run -- gateway --http --http-port 3000
```

1. Fork the repository
2. Create a feature branch (`git checkout -b feat/my-feature`)
3. Write tests for your changes
4. Submit a pull request

---

## License

[MIT](LICENSE) -- Copyright (c) 2025-2026 nanobot contributors

---

<div align="center">

**If nanobot helps you, consider giving it a star!**

[![Star History Chart](https://api.star-history.com/svg?repos=yukihamada/nanobot&type=Date)](https://star-history.com/#yukihamada/nanobot&Date)

</div>
