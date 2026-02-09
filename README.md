<div align="center">

# nanobot

[![Build](https://github.com/yukihamada/nanobot/actions/workflows/ci.yml/badge.svg)](https://github.com/yukihamada/nanobot/actions)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![GitHub stars](https://img.shields.io/github/stars/yukihamada/nanobot?style=social)](https://github.com/yukihamada/nanobot/stargazers)
[![Rust](https://img.shields.io/badge/Built_with-Rust-dea584?logo=rust)](https://www.rust-lang.org/)

**AI Agent Platform** — Deploy intelligent AI agents to LINE, Telegram, Web in minutes.

[Try it now](https://chatweb.ai) | [API Docs](https://teai.io) | [iOS App](https://apps.apple.com/app/eliochat/id6742071881)

</div>

---

## Why nanobot?

Most AI chatbot frameworks are slow Python wrappers around a single model. nanobot is different: a **production-grade Rust runtime** that connects any LLM to any channel, with voice, tools, and memory built in.

| | nanobot | Typical framework |
|---|---|---|
| Language | Rust (cold start < 50ms) | Python (cold start 3-10s) |
| Channels | Web + LINE + Telegram + Facebook | Usually one |
| Models | GPT-4o, Claude, Gemini (hot-swap) | Single provider |
| Voice | STT + TTS, push-to-talk | None |
| Memory | Cross-channel long-term memory | Session only |
| Deploy | Lambda / Fly.io / Docker / one-click | "figure it out" |

---

## Features

- **Voice-First** — Push-to-talk microphone UI with speech-to-text and auto-TTS response. Talk to your AI, don't type.
- **Multi-Model** — GPT-4o, Claude Sonnet/Opus, Gemini Flash/Pro. Automatic model selection per channel, or pick your own.
- **Multi-Channel** — Web, LINE, Telegram, Facebook Messenger. One conversation synced across all channels with `/link`.
- **MCP Tools** — Web search, weather, calculator, page fetch, and custom tool calling. The agent decides when to use tools.
- **Long-Term Memory** — OpenClaw-inspired daily logs and long-term memory stored in DynamoDB, injected into every conversation.
- **Serverless Rust** — Compiled to ARM64, runs on AWS Lambda with sub-50ms cold starts. Handles 1,000+ concurrent sessions.

---

## Quick Start

### Chat via API (no auth required)

```bash
curl -X POST https://chatweb.ai/api/v1/chat \
  -H "Content-Type: application/json" \
  -d '{"message": "Hello! What can you do?", "session_id": "my-session"}'
```

### Stream responses (SSE)

```bash
curl -N https://chatweb.ai/api/v1/chat/stream \
  -H "Content-Type: application/json" \
  -d '{"message": "Write me a haiku about Rust", "session_id": "my-session"}'
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
memory      Google      web_fetch
```

**CI/CD Pipeline:** Push to `main` &rarr; Test &rarr; Build &rarr; Canary 10% &rarr; Production 100%

---

## Self-Hosting

### One-Click Deploy

| Platform | |
|----------|---|
| **Railway** | [![Deploy on Railway](https://railway.app/button.svg)](https://railway.app/template/nanobot) |
| **Render** | [![Deploy to Render](https://render.com/images/deploy-to-render-button.svg)](https://render.com/deploy?repo=https://github.com/yukihamada/nanobot) |
| **Koyeb** | [![Deploy to Koyeb](https://www.koyeb.com/static/images/deploy/button.svg)](https://app.koyeb.com/deploy?type=git&repository=yukihamada/nanobot) |

### Docker

```bash
docker run -p 3000:3000 \
  -e ANTHROPIC_API_KEY=sk-... \
  ghcr.io/yukihamada/nanobot
```

### AWS Lambda (ARM64)

```bash
brew install zig && cargo install cargo-zigbuild
rustup target add aarch64-unknown-linux-gnu

cargo zigbuild \
  --manifest-path crates/nanobot-lambda/Cargo.toml \
  --release --target aarch64-unknown-linux-gnu

cp target/aarch64-unknown-linux-gnu/release/bootstrap ./bootstrap
zip -j lambda.zip bootstrap
aws lambda update-function-code --function-name nanobot --zip-file fileb://lambda.zip
```

### Fly.io

```bash
fly launch --no-deploy && fly deploy
```

### Local Development

```bash
cargo run -- gateway --http --http-port 3000
# Open http://localhost:3000
```

---

## Channel Sync

Conversations stay in sync across all channels:

1. **Session ID** — Copy your `webchat:xxxx-...` ID from the web UI and send it in LINE/Telegram to auto-link.
2. **Link Code** — Send `/link` in any channel to get a 6-digit code. Send `/link CODE` in another channel to merge.
3. **Deep Link** — Tap the LINE/Telegram button in the web UI for instant linking.

---

## API Reference

| Method | Endpoint | Description |
|--------|----------|-------------|
| `POST` | `/api/v1/chat` | Send a message, get an AI response |
| `POST` | `/api/v1/chat/stream` | SSE streaming response |
| `POST` | `/api/v1/speech/synthesize` | Text-to-speech (MP3) |
| `GET` | `/api/v1/sessions/{id}` | Get session with history |
| `GET` | `/api/v1/sessions` | List sessions |
| `DELETE` | `/api/v1/sessions/{id}` | Delete a session |
| `GET` | `/api/v1/account/{id}` | User profile, plan, credits |
| `GET` | `/api/v1/providers` | Available AI providers |
| `GET` | `/api/v1/integrations` | Available tools and integrations |
| `GET` | `/health` | Health check |

Full API docs at [teai.io](https://teai.io).

---

## Project Structure

```
crates/
  nanobot-core/      Core library (channels, AI providers, sessions, HTTP API)
  nanobot-lambda/    AWS Lambda handler
infra/               SAM templates, deploy scripts
web/                 Frontend (index.html, pricing.html, etc.)
src/                 Local server CLI
tests/               Integration tests
```

---

## Pricing

| Plan | Price | What you get |
|------|-------|--------------|
| **Free** | $0/mo | 1,000 credits, GPT-4o-mini, Gemini Flash |
| **Starter** | $9/mo | 25,000 credits + GPT-4o, Claude Sonnet |
| **Pro** | $29/mo | 300,000 credits + Claude Opus, all models |

Use coupon code `LAUNCH2026` for a free first month on Starter.

---

## Ecosystem

| Product | Description | Link |
|---------|-------------|------|
| **chatweb.ai** | Consumer AI assistant (Web, LINE, Telegram) | [chatweb.ai](https://chatweb.ai) |
| **teai.io** | Developer platform and API | [teai.io](https://teai.io) |
| **ElioChat** | Offline-capable iOS AI companion | [App Store](https://apps.apple.com/app/eliochat/id6742071881) |

---

## Contributing

Contributions are welcome! Here's how to get started:

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

Please open an issue first for large changes so we can discuss the approach.

---

## License

[MIT](LICENSE) -- Copyright (c) 2025-2026 nanobot contributors

---

<div align="center">

**If nanobot helps you, consider giving it a star!**

[![Star History Chart](https://api.star-history.com/svg?repos=yukihamada/nanobot&type=Date)](https://star-history.com/#yukihamada/nanobot&Date)

</div>
