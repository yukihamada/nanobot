<div align="center">

<img src="docs/images/hero.png" alt="nanobot — Boot in 0.1s! Hosted at chatweb.ai" width="700" />

# nanobot

### Production-Ready AI Agent Platform in Pure Rust 🦀

> **Fork notice:** This is a complete rewrite of [HKUDS/nanobot](https://github.com/HKUDS/nanobot) (Python) in Rust — same philosophy of minimal, hackable AI agents, rebuilt for production scale and voice-first deployment.

[![CI](https://github.com/yukihamada/nanobot/actions/workflows/ci.yml/badge.svg)](https://github.com/yukihamada/nanobot/actions/workflows/ci.yml)
[![Deploy](https://github.com/yukihamada/nanobot/actions/workflows/deploy.yml/badge.svg)](https://github.com/yukihamada/nanobot/actions/workflows/deploy.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.75+-dea584?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Release](https://img.shields.io/github/v/release/yukihamada/nanobot?color=green)](https://github.com/yukihamada/nanobot/releases)
[![Stars](https://img.shields.io/github/stars/yukihamada/nanobot?style=social)](https://github.com/yukihamada/nanobot/stargazers)

**Your personal AI assistant that runs anywhere.**
Voice-first • 13 channels • 35 tools (24 core + 11 optional) • Multi-model failover • <50ms cold start

**🌐 [chatweb.ai](https://chatweb.ai)** — Voice-first AI assistant for everyone
**🛠️ [teai.io](https://teai.io)** — Developer API (same backend)

<br/>

**[🚀 Try chatweb.ai](https://chatweb.ai)** · **[📚 Documentation](https://chatweb.ai/docs)** · **[📊 Compare](https://chatweb.ai/comparison)** · **[⚡ API Docs (teai.io)](https://teai.io)**

<br/>

</div>

---

## ✨ Highlights

<table>
<tr>
<td width="50%">

### ⚡ Blazing Fast
**<50ms cold start** on AWS Lambda ARM64
20x faster than Node.js agents
Single binary (~24 MB ARM64)

</td>
<td width="50%">

### 🗣️ Voice-First
Native **STT + TTS** with push-to-talk UI
Auto-TTS reads responses aloud
The only open-source agent with full voice

</td>
</tr>
<tr>
<td>

### 📱 13 Channels
Web · LINE · Telegram · Discord
Slack · Teams · Facebook · and more
One conversation, synced everywhere

</td>
<td>

### 🛠️ 35 Built-in Tools
Web search · Code execution · File ops
Image/Music/Video generation
GitHub · Gmail · Calendar · PostgreSQL
**Self-improvement**: `/improve` command

</td>
</tr>
<tr>
<td>

### 🔄 Auto Failover
**Parallel provider racing with fallback**
Zero user-visible errors
Multi-key load balancing

</td>
<td>

### 🧠 Long-Term Memory
**2-layer auto-consolidation**
Session → Daily → Long-term
Context persists across channels

</td>
</tr>
</table>

---

## 🌍 Multi-Language Support

<div align="center">

| 🇯🇵 日本語 | 🇺🇸 English | 🇨🇳 中文 | 🇰🇷 한국어 | 🇪🇸 Español |
|:---:|:---:|:---:|:---:|:---:|
| [Web](https://chatweb.ai?lang=ja) | [Web](https://chatweb.ai?lang=en) | [网页](https://chatweb.ai?lang=zh) | [웹](https://chatweb.ai?lang=ko) | [Web](https://chatweb.ai?lang=es) |
| [LINE Bot](https://line.me/R/ti/p/@619jcqqh) | [Telegram](https://t.me/chatweb_ai_bot) | [Telegram](https://t.me/chatweb_ai_bot) | [Telegram](https://t.me/chatweb_ai_bot) | [Telegram](https://t.me/chatweb_ai_bot) |

**AI responds in 100+ languages** • **UI available in 7 languages** (🇯🇵 🇺🇸 🇨🇳 🇰🇷 🇪🇸 🇫🇷 🇩🇪)

</div>

---

## 🚀 Quick Start

### Try Without Installation

```bash
# chatweb.ai (recommended for general use)
curl -X POST https://chatweb.ai/api/v1/chat \
  -H "Content-Type: application/json" \
  -d '{"message": "Hello from nanobot!", "session_id": "demo"}'

# teai.io (developer-focused, same API)
# curl -X POST https://teai.io/api/v1/chat ...
```

### Run Locally (Docker)

```bash
docker run -p 3000:3000 \
  -e OPENAI_API_KEY=sk-... \
  ghcr.io/yukihamada/nanobot

# Open http://localhost:3000
```

### Build from Source

```bash
git clone https://github.com/yukihamada/nanobot.git
cd nanobot
export OPENAI_API_KEY=sk-...

# Option 1: Run as HTTP gateway (Web UI)
cargo run --release -- gateway --http --http-port 3000

# Option 2: Run as CLI (Interactive terminal)
cargo run --release -- chat --model claude-sonnet-4-5

# Option 3: Execute single command
echo "Search for Rust async best practices" | cargo run --release -- chat --model claude-sonnet-4-5
```

**CLI Mode Features:**
- Interactive conversation with autonomous coding agent
- Access to all 30 tools (git, linter, tests, file ops, web search)
- Self-correction loop: automatically fixes linter/test errors
- OODA Loop: systematic approach to complex tasks
- Workspace memory: persistent context in `~/.nanobot/workspace/memory/`

**Quick Test:**
```bash
cargo run --release -- chat --model claude-sonnet-4-5 <<< "Show git status and run tests for this project"
```

**After Building:**
```bash
# Binary location: target/release/chatweb
./target/release/chatweb chat  # Interactive mode
./target/release/chatweb chat "Your message here"  # Single command

# Or install globally:
cargo install --path .
chatweb chat  # Now available from anywhere
```

**Environment Variables:**
- `OPENAI_API_KEY` (required): Your OpenAI API key
- `ANTHROPIC_API_KEY` (optional): For Claude models (recommended)
- `NANOBOT_WORKSPACE` (optional): Workspace directory (default: `~/.nanobot/workspace`)

### Supported LLM Providers

nanobot supports **8+ LLM providers** with automatic API base detection:

| Provider | API Key | Models | Notes |
|----------|---------|--------|-------|
| **OpenAI** | `OPENAI_API_KEY` | GPT-4o, GPT-4, GPT-3.5 | Default provider |
| **Anthropic** | `ANTHROPIC_API_KEY` | Claude Opus/Sonnet/Haiku | Recommended for coding |
| **Google** | `GOOGLE_API_KEY` | Gemini Pro/Flash | Free tier available |
| **OpenRouter** | `OPENROUTER_API_KEY` | 100+ models | Multi-provider aggregator |
| **DeepSeek** | `DEEPSEEK_API_KEY` | DeepSeek-V3 | Chinese provider, coding-focused |
| **Moonshot** | `MOONSHOT_API_KEY` | Moonshot-v1, Kimi | Chinese provider, long context |
| **Qwen** | `QWEN_API_KEY` | Qwen-Max, Qwen-Plus | Alibaba Cloud (通义千问) |
| **MiniMax** | `MINIMAX_API_KEY` | MiniMax models | Chinese provider |

**Auto-detection**: Just set the API key and use model names like `deepseek/deepseek-chat` or `qwen/qwen-max`.

**Example:**
```bash
export DEEPSEEK_API_KEY=sk-...
cargo run -- chat --model deepseek/deepseek-chat
```

<details>
<summary><b>Deploy to AWS Lambda (Production)</b></summary>

```bash
# Install cross-compilation tools
brew install zig && cargo install cargo-zigbuild
rustup target add aarch64-unknown-linux-gnu

# Build for Lambda ARM64
cargo zigbuild --manifest-path crates/nanobot-lambda/Cargo.toml \
  --release --target aarch64-unknown-linux-gnu

# Deploy with SAM
cd infra && sam build && sam deploy --guided
```

See [deployment guide](docs/deployment.md) for details.

</details>

---

## 📊 Comparison

<div align="center">
<img src="docs/images/comparison.png" alt="nanobot vs PicoClaw vs openClaw comparison" width="800" />
</div>

### nanobot (Rust) vs HKUDS/nanobot (Python)

| | **yukihamada/nanobot** (Rust) | [HKUDS/nanobot](https://github.com/HKUDS/nanobot) (Python) |
|---|:---:|:---:|
| **Language** | Rust | Python |
| **Binary Size** | ~50 MB | ~4,000 lines (interpreter required) |
| **Cold Start** | **0.1s** | ~3–5s |
| **Voice** | ✅ Native STT/TTS | ❌ |
| **Self-Improving** | ✅ `/improve` command | ❌ |
| **Channels** | 13 (LINE, Telegram, Discord…) | 9+ (Telegram, Discord, Slack…) |
| **LLM Providers** | 8+ with auto failover | 13+ (no failover) |
| **Hosted Service** | ✅ chatweb.ai / teai.io | ❌ self-host only |
| **Target** | Production / Voice-first | Research / Hackable |

### nanobot vs Others

| | **nanobot** | openClaw | PicoClaw |
|---|:---:|:---:|:---:|
| **Price** | **$5 (hardware)** | $599 (Mac Mini) | $10 (hardware) |
| **Binary Size** | **~50 MB** | >1000 MB | <10 MB |
| **Cold Start** | **0.1s** | ~1s | <1s |
| **Language** | Rust | TypeScript | Go |
| **Self-Improving** | ✅ | ❌ | ❌ |
| **Voice** | ✅ Native | ⚠️ Partial | ❌ |
| **Auto Failover** | ✅ | ❌ | ❌ |

**Runs on any Linux board as low as $5** (Raspberry Pi Zero, LicheeRV Nano, etc.)
> **ESP32 support (experimental):** `no_std` + Rust on ESP32-C3 は理論上動作可能。未検証ですが、挑戦者募集中！ 🦐

**ハードウェア不要で今すぐ試す:**
- 🌐 **[chatweb.ai](https://chatweb.ai)** — 音声AIアシスタント（フリーミアム、登録不要でお試し可）
- 🛠️ **[teai.io](https://teai.io)** — 開発者向けAPI（フリーミアム、無料枠あり）

**Origins:** Complete Rust rewrite of [HKUDS/nanobot](https://github.com/HKUDS/nanobot), inspired by [sipeed/picoclaw](https://github.com/sipeed/picoclaw).

[View full comparison →](https://chatweb.ai/comparison)

---

## 🛠️ Built-in Tools

<details>
<summary><b>Core Tools (8)</b></summary>

- `web_search` - Brave/Bing/Jina 3-tier fallback
- `web_fetch` - Jina Reader for JS-heavy pages
- `browser` - CSS selector, screenshots, forms
- `code_execute` - Sandboxed shell/Python/Node.js
- `calculator` - Math expressions
- `weather` - Global weather data
- `wikipedia` - Encyclopedia lookup
- `translation` - Multi-language

</details>

<details>
<summary><b>File & Workspace (4)</b></summary>

- `file_read` / `file_write` / `file_list`
- `filesystem` - Find (glob) + grep (regex)

</details>

<details>
<summary><b>Content Creation (4)</b></summary>

- `image_generate` - OpenAI DALL-E
- `music_generate` - Suno API
- `video_generate` - Kling API
- `qr_code` - QR code generation

</details>

<details>
<summary><b>Integrations (15, API key required)</b></summary>

**Data & Research**
- `news_search`, `youtube_transcript`, `arxiv_search`, `csv_analysis`

**Productivity**
- `google_calendar`, `gmail`, `slack`, `discord`, `notion`, `postgresql`, `spotify`

**Development**
- `github` (read/write files, create PRs), `webhook`, `phone_call`, `web_deploy`

</details>

---

## 🏗️ Architecture

```mermaid
graph TB
    subgraph Channels
        WEB[Web SPA]
        LINE[LINE]
        TG[Telegram]
        WA[WhatsApp]
        DISCORD[Discord]
        SLACK[Slack]
    end

    subgraph Runtime["nanobot (Rust)"]
        ROUTER[HTTP Router]
        AUTH[Auth & Credits]
        AGENT[Agentic Loop]
        VOICE[STT / TTS]
    end

    subgraph Providers
        ANTHROPIC[Claude]
        OPENAI[GPT-4o]
        GEMINI[Gemini]
    end

    Channels --> ROUTER
    ROUTER --> AUTH
    ROUTER --> AGENT
    ROUTER --> VOICE
    AGENT --> Providers
    AGENT --> DDB[(DynamoDB)]
```

**Design Philosophy:**
- **Self-Hostable First** - Own your data
- **Voice-Native** - Push-to-talk as primary interaction
- **Multi-Channel** - Users stay in their favorite apps
- **Automatic Failover** - Infrastructure failures are invisible
- **MCP-Compatible** - Standard AI-to-AI protocols

---

## 📦 System Requirements

| Component | Minimum | Recommended |
|-----------|---------|-------------|
| **CPU** | 1 core | 2+ cores |
| **RAM** | 128 MB | 512 MB |
| **Disk** | 20 MB | 100 MB |

**Supported Platforms:** Linux (x86_64, ARM64), macOS (11+), Windows (WSL2), AWS Lambda

---

## 🔐 Security

- **Sandboxed execution** - All code runs in isolated `/tmp/sandbox/{session_id}/`
- **Password hashing** - HMAC-SHA256 with configurable keys
- **Rate limiting** - 5 login attempts/min, 3 registrations/min
- **Webhook verification** - Telegram, Facebook, Stripe signature validation
- **Audit logging** - 90-day TTL in DynamoDB
- **CORS restrictions** - Whitelist-only

See [SECURITY.md](SECURITY.md) for vulnerability reporting.

---

## 🗺️ Roadmap

- [x] Multi-model failover
- [x] Voice-first UI with STT/TTS
- [x] 13 channel integrations
- [x] 35 built-in tools (24 core + 11 optional)
- [x] MCP server support
- [x] Stripe billing
- [x] 7-language UI support
- [ ] WebSocket streaming (Q2 2026)
- [ ] Multi-agent orchestration (Q2 2026)
- [ ] Custom skill marketplace (Q3 2026)
- [ ] On-device LLM inference (Q3 2026)

---

## 🤝 Contributing

We welcome contributions! See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

```bash
# Fork, clone, and test
git clone https://github.com/YOUR_USERNAME/nanobot.git
cd nanobot
cargo test --all
cargo clippy --all-targets
```

---

## 📄 License

[MIT License](LICENSE) - Copyright (c) 2025-2026 nanobot contributors

---

## 🙏 Acknowledgments

Built on the shoulders of giants:
- [HKUDS/nanobot](https://github.com/HKUDS/nanobot) - Original Python nanobot (this project is a complete Rust rewrite)
- [sipeed/picoclaw](https://github.com/sipeed/picoclaw) - MCP architecture inspiration
- [openclaw/openclaw](https://github.com/openclaw/openclaw) - Multi-channel patterns
- Anthropic Claude, OpenAI, Google Gemini - LLM providers
- Rust community - axum, tokio, serde, and countless crates

---

<div align="center">

**Built with Rust 🦀 · Deployed on Lambda ⚡ · Scales to millions 📈**

---

### 🌐 Our Services

**[chatweb.ai](https://chatweb.ai)** — Voice-first AI assistant for everyone
**[teai.io](https://teai.io)** — Developer-focused API (same backend)

Both powered by nanobot • Same features • Same API

---

[![Star History Chart](https://api.star-history.com/svg?repos=yukihamada/nanobot&type=Date)](https://star-history.com/#yukihamada/nanobot&Date)

Made with ❤️ in Japan 🇯🇵

</div>
