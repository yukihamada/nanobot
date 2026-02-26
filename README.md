<div align="center">

# nanobot

**A production-grade AI agent platform written in pure Rust.**

One binary. Six channels. Fifty tools. Zero cold-start drama.

[![CI](https://github.com/yukihamada/nanobot/actions/workflows/ci.yml/badge.svg)](https://github.com/yukihamada/nanobot/actions/workflows/ci.yml)
[![Deploy](https://github.com/yukihamada/nanobot/actions/workflows/deploy.yml/badge.svg)](https://github.com/yukihamada/nanobot/actions/workflows/deploy.yml)
[![Rust](https://img.shields.io/badge/Rust-1.75+-dea584?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Release](https://img.shields.io/github/v/release/yukihamada/nanobot?color=green)](https://github.com/yukihamada/nanobot/releases)
[![Stars](https://img.shields.io/github/stars/yukihamada/nanobot?style=social)](https://github.com/yukihamada/nanobot/stargazers)

[Live Demo](https://chatweb.ai) &middot; [API Docs](https://teai.io) &middot; [Report Bug](https://github.com/yukihamada/nanobot/issues)

</div>

---

nanobot is a self-hostable, multi-channel AI assistant that ships as a single Rust binary. It connects to **8+ LLM providers** with automatic failover, exposes **50+ agentic tools**, and deploys to AWS Lambda for pennies. It powers [chatweb.ai](https://chatweb.ai) and [teai.io](https://teai.io) in production today.

## Why nanobot?

| | nanobot | Typical agent frameworks |
|---|---|---|
| **Language** | Rust (axum) | Python / TypeScript |
| **Cold start** | < 50 ms on Lambda ARM64 | 3-10 s |
| **Binary** | ~9 MB stripped | Hundreds of MB + runtime |
| **Channels** | Web, LINE, Telegram, Discord, Slack, Facebook | Usually 1-2 |
| **LLM failover** | Automatic round-robin + circuit breaker | Manual config |
| **Voice** | Built-in STT + TTS | External service required |
| **Self-host** | Single binary, zero dependencies | Docker + DB + queue + ... |
| **License** | MIT | Varies |

---

## Architecture

```
                         +------------------+
                         |   Your users     |
                         +--------+---------+
                                  |
            +----------+----------+----------+----------+
            |          |          |          |          |
          Web       LINE    Telegram    Discord    Slack ...
            |          |          |          |          |
            +----------+----------+----------+----------+
                                  |
                       +----------v----------+
                       |   API Gateway /     |
                       |   Reverse Proxy     |
                       +----------+----------+
                                  |
                       +----------v----------+
                       |     nanobot         |
                       |  (single binary)    |
                       |                     |
                       |  +-- Auth & Credits |
                       |  +-- Agentic Loop   |
                       |  +-- Tool Runtime   |
                       |  +-- STT / TTS      |
                       |  +-- Memory Engine  |
                       +----+------+----+----+
                            |      |    |
               +------------+   +--+    +------------+
               |                |                    |
        +------v------+  +-----v------+  +---------v---------+
        | LLM Providers|  |  DynamoDB  |  |   External APIs   |
        | (8+ w/ fail- |  | (sessions, |  | (Brave, Jina,     |
        |  over)       |  |  memory,   |  |  OpenAI TTS, ...) |
        +--------------+  |  credits)  |  +-------------------+
                          +------------+
```

---

## Features

### Multi-LLM with Automatic Failover

nanobot doesn't lock you into a single provider. Configure multiple API keys and it handles the rest -- round-robin load balancing, circuit breakers, and transparent failover across providers.

| Provider | Models | Notes |
|----------|--------|-------|
| **OpenRouter** | 100+ models | Aggregator -- single key, all models |
| **Anthropic** | Claude Opus / Sonnet / Haiku | Recommended for reasoning |
| **OpenAI** | GPT-4o, o4-mini | Broad tool support |
| **Google** | Gemini 2.5 Pro / Flash | Free tier available |
| **DeepSeek** | DeepSeek-V3 | Strong at code |
| **Moonshot** | Kimi-K2.5 | Long context |
| **Qwen** | Qwen-Max, Qwen-Plus | Alibaba Cloud |
| **MiniMax** | MiniMax-M2.5 | Fast inference |

Tiered model selection (economy / normal / powerful) lets you balance cost and quality per request.

### 50+ Built-in Tools

Agentic mode executes multi-step tool chains automatically. Free users get 1 iteration; Pro users get up to 5 with parallel tool execution.

<details>
<summary><strong>Core (always available)</strong></summary>

| Tool | Description |
|------|-------------|
| `web_search` | Brave / Bing / Jina 3-tier fallback |
| `web_fetch` | Jina Reader for JS-heavy pages |
| `browser` | CSS selector queries, screenshots, forms |
| `code_execute` | Sandboxed shell execution |
| `calculator` | Arbitrary math expressions |
| `weather` | Global weather data |
| `wikipedia` | Encyclopedia lookup |
| `translation` | Multi-language translation |
| `datetime` | Time zones, date math |
| `qr_code` | QR code generation |
| `file_read` / `file_write` / `file_list` | Workspace file operations |
| `filesystem` | Glob find + regex grep |
| `csv_analysis` | Summary, filter, aggregate |
| `image_generate` | DALL-E image generation |
| `music_generate` | Suno API |
| `video_generate` | Kling API |

</details>

<details>
<summary><strong>Integrations (API key required)</strong></summary>

| Tool | Description |
|------|-------------|
| `github` | Read/write files, create PRs |
| `gmail` | Send and search email |
| `google_calendar` | Event management |
| `slack` | Post and search messages |
| `discord` | Channel messaging |
| `notion` | Page and database queries |
| `spotify` | Playback control, search |
| `postgresql` | Direct SQL queries |
| `youtube_transcript` | Video transcript extraction |
| `arxiv_search` | Academic paper search |
| `news_search` | News aggregation |
| `webhook` | Trigger arbitrary webhooks |
| `phone_call` | Amazon Connect integration |
| `web_deploy` | One-click static site deploy |

</details>

<details>
<summary><strong>Developer tools (CLI / workspace mode)</strong></summary>

| Tool | Description |
|------|-------------|
| `git_status` | Working tree status |
| `git_diff` | Staged/unstaged diffs |
| `git_commit` | Commit with message |
| `run_linter` | Clippy / ESLint / etc. |
| `run_tests` | Run project test suite |

</details>

### Skill Marketplace

Users can publish and install custom skills:

- **Prompt skills** -- inject system prompts for specialized personas or domain knowledge
- **Tool skills** -- expose any HTTPS endpoint as an LLM-callable tool via webhook

Skills are stored in DynamoDB and loaded at chat time. No redeploy required.

### Multi-Channel

One codebase serves all channels. Conversations sync across them.

| Channel | Status | Optimizations |
|---------|--------|---------------|
| **Web** (SPA) | Production | Voice-first UI, SSE streaming, auto-TTS |
| **LINE** | Production | 200-char responses, emoji, bullet points |
| **Telegram** | Production | 300-char responses, Markdown formatting |
| **Facebook Messenger** | Production | 300-char concise replies |
| **Discord** | Production | Webhook integration |
| **Slack** | Production | Bot token integration |

### Voice-First

- **STT**: Web Speech API (browser-side, zero server cost)
- **TTS**: OpenAI `tts-1` with response caching
- **Auto-TTS**: Voice input triggers automatic voice output
- Push-to-talk UI with visual feedback

### Long-Term Memory

Two-layer auto-consolidation inspired by [OpenClaw](https://github.com/openclaw/openclaw):

```
Session context (20 messages)
        |
        v
Daily log (auto-appended after each conversation)
        |
        v
Long-term memory (consolidated summaries)
```

Memory persists across channels and sessions via DynamoDB.

### A/B Testing Framework

Built-in CRO experimentation:

- Deterministic variant assignment (`hash(uid + testId) % N`)
- Event tracking via `POST /api/v1/ab/event`
- Aggregated stats with 90-day TTL
- No external analytics dependency

---

## Quick Start

### Try the hosted API (no setup)

```bash
curl -X POST https://chatweb.ai/api/v1/chat \
  -H "Content-Type: application/json" \
  -d '{"message": "What can you do?", "session_id": "demo"}'
```

### Run locally

```bash
git clone https://github.com/yukihamada/nanobot.git
cd nanobot

# Set at least one provider key
export ANTHROPIC_API_KEY=sk-ant-...
# or: export OPENAI_API_KEY=sk-...
# or: export OPENROUTER_API_KEY=sk-or-...

# Build and run the web gateway
cargo build --bin chatweb
./target/debug/chatweb gateway --http --http-port 3000
# Open http://localhost:3000
```

### Docker

```bash
docker run -p 3000:3000 \
  -e OPENAI_API_KEY=sk-... \
  ghcr.io/yukihamada/nanobot
```

### CLI usage

```bash
# Interactive agent mode (uses your local API keys directly)
./target/debug/chatweb agent

# Single-shot message
./target/debug/chatweb agent -m "Summarize today's tech news"

# Check configuration
./target/debug/chatweb status

# Install globally
cargo install --path .
chatweb agent
```

### Deploy to AWS Lambda

```bash
# Prerequisites
brew install zig && cargo install cargo-zigbuild
rustup target add aarch64-unknown-linux-musl

# Build for Lambda ARM64 (must use musl, not gnu)
cargo zigbuild --manifest-path crates/nanobot-lambda/Cargo.toml \
  --release --target aarch64-unknown-linux-musl

# Or use the deploy script
LAMBDA_FUNCTION_NAME=nanobot-prod ./infra/deploy-fast.sh
```

---

## Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `ANTHROPIC_API_KEY` | One of these | Claude models |
| `OPENAI_API_KEY` | | GPT-4o and TTS |
| `OPENROUTER_API_KEY` | | 100+ models via single key |
| `GOOGLE_API_KEY` | | Gemini models |
| `DEEPSEEK_API_KEY` | | DeepSeek-V3 |
| `LINE_CHANNEL_SECRET` | For LINE | LINE Messaging API |
| `TELEGRAM_BOT_TOKEN` | For Telegram | Telegram Bot API |
| `STRIPE_SECRET_KEY` | For billing | Stripe integration |
| `NANOBOT_WORKSPACE` | No | Workspace directory (default: `~/.nanobot/workspace`) |

---

## API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/api/v1/chat` | Send a message, get a response |
| `POST` | `/api/v1/chat/stream` | SSE streaming response |
| `POST` | `/api/v1/chat/race` | Multi-model race (economy/normal/powerful) |
| `POST` | `/api/v1/chat/explore` | Parallel execution across all models |
| `POST` | `/api/v1/speech/synthesize` | Text-to-speech |
| `GET`  | `/api/v1/auth/me` | Current user info |
| `GET`  | `/api/v1/skills` | Browse skill marketplace |
| `POST` | `/api/v1/skills/publish` | Publish a custom skill |
| `POST` | `/api/v1/coupon/redeem` | Apply coupon code |
| `POST` | `/webhooks/line` | LINE webhook |
| `POST` | `/webhooks/telegram` | Telegram webhook |
| `POST` | `/webhooks/stripe` | Stripe webhook |

---

## System Requirements

| | Minimum | Recommended |
|---|---------|-------------|
| **CPU** | 1 core | 2+ cores |
| **RAM** | 128 MB | 512 MB |
| **Disk** | 20 MB | 100 MB |

**Platforms**: Linux (x86_64, ARM64), macOS (Apple Silicon, Intel), Windows (WSL2), AWS Lambda (ARM64)

---

## Security

- **Sandboxed execution** -- tool code runs in isolated `/tmp/sandbox/{session_id}/`
- **HMAC-SHA256 password hashing** with configurable keys
- **Rate limiting** -- 5 login attempts/min, 3 registrations/min
- **Webhook signature verification** -- Telegram, Facebook, Stripe
- **Audit logging** -- 90-day TTL in DynamoDB
- **CORS whitelist** -- only configured origins allowed

See [SECURITY.md](SECURITY.md) for vulnerability reporting.

---

## Roadmap

- [x] Multi-model failover with circuit breakers
- [x] Voice-first UI (STT + TTS)
- [x] 6 channel integrations (Web, LINE, Telegram, Discord, Slack, Facebook)
- [x] 50+ built-in tools with agentic loop
- [x] Skill marketplace (publish and install custom tools)
- [x] A/B testing framework
- [x] Stripe billing integration
- [x] Long-term memory engine
- [x] SSE streaming
- [ ] WebSocket transport (Q2 2026)
- [ ] Multi-agent orchestration (Q2 2026)
- [ ] On-device LLM inference via GGUF (Q3 2026)

---

## Project Structure

```
nanobot/
  crates/
    nanobot-core/         Core library: handlers, tools, providers, memory
    nanobot-lambda/       AWS Lambda entrypoint
  nanobot-cli/            CLI binary
  web/
    index.html            Web SPA (embedded into binary via include_str!)
    skill.html            Skill marketplace UI
    pricing.html          Pricing page
  infra/
    deploy-fast.sh        One-command Lambda deploy
    template.yaml         SAM template
```

---

## Contributing

```bash
git clone https://github.com/YOUR_USERNAME/nanobot.git
cd nanobot
cargo test --all
cargo clippy --all-targets
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

---

## License

[MIT](LICENSE) -- Copyright (c) 2025-2026 nanobot contributors

---

## Acknowledgments

- [HKUDS/nanobot](https://github.com/HKUDS/nanobot) -- original Python nanobot (this project is a complete Rust rewrite)
- [axum](https://github.com/tokio-rs/axum), [tokio](https://tokio.rs/), [serde](https://serde.rs/) -- the Rust ecosystem that makes this possible
- Anthropic, OpenAI, Google -- LLM providers

---

<div align="center">

**[chatweb.ai](https://chatweb.ai)** -- voice-first AI assistant &middot; **[teai.io](https://teai.io)** -- developer API

Both powered by nanobot.

[![Star History Chart](https://api.star-history.com/svg?repos=yukihamada/nanobot&type=Date)](https://star-history.com/#yukihamada/nanobot&Date)

</div>
