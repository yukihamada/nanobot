# nanobot

AI Agent Platform — Deploy intelligent AI agents to LINE and Telegram in minutes.

**Live:** [https://chatweb.ai](https://chatweb.ai)

## Features

- **Multi-Model** — GPT-4o, Claude, Gemini. Switch anytime.
- **Multi-Channel** — LINE, Telegram, REST API
- **Serverless** — Rust on AWS Lambda. Sub-second cold starts.
- **Persistent Memory** — Context across conversations
- **Freemium** — Start free, scale as you grow

## Quick Start

```bash
# Chat via API
curl -X POST https://api.chatweb.ai/api/v1/chat \
  -H "Content-Type: application/json" \
  -d '{"message": "Hello!", "session_id": "user-123"}'
```

## Architecture

```
LINE / Telegram / API
        │
   API Gateway
        │
   AWS Lambda (Rust, ARM64)
        │
   DynamoDB (sessions, memory, config)
```

## Deploy

```bash
# Prerequisites
brew install aws-sam-cli zig
cargo install cargo-zigbuild
rustup target add aarch64-unknown-linux-gnu

# Deploy
./infra/deploy.sh

# Set up webhooks
./infra/setup-webhook.sh
```

## Development

```bash
cargo test -p nanobot-core
cargo run  # local server on :3000
```

## License

MIT
