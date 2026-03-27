# nanobot

A production-grade AI agent platform written in pure Rust.

One binary. Six channels. Fifty tools. Zero cold-start drama.

[![CI](https://github.com/yukihamada/nanobot/actions/workflows/ci.yml/badge.svg)](https://github.com/yukihamada/nanobot/actions/workflows/ci.yml)
[![Deploy](https://github.com/yukihamada/nanobot/actions/workflows/deploy.yml/badge.svg)](https://github.com/yukihamada/nanobot/actions/workflows/deploy.yml)
[![Rust](https://img.shields.io/badge/Rust-1.75+-dea584?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Release](https://img.shields.io/github/v/release/yukihamada/nanobot?color=green)](https://github.com/yukihamada/nanobot/releases)
[![Stars](https://img.shields.io/github/stars/yukihamada/nanobot?style=social)](https://github.com/yukihamada/nanobot/stargazers)

[Live Demo](https://chatweb.ai) · [API Docs](https://teai.io) · [Report Bug](https://github.com/yukihamada/nanobot/issues)

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

nanobot doesn’t lock you into a single provider. Configure multiple API keys and it handles the rest -- round-robin load balancing, circuit breakers, and transparent failover across providers.

| Provider | API Keys |
| --- | --- |
| AI21 Labs | 2 |
| Anthropic | 1 |
| Google Cloud AI | 1 |
| Hugging Face | 1 |
| Meta AI | 1 |
| Microsoft Azure | 1 |
| NVIDIA AI | 1 |
| OpenAI | 2 |

### Agentic Tools

| Tool | Description |
| --- | --- |
| Sentiment Analysis | Analyze text sentiment |
| Named Entity Recognition | Extract entities from text |
| Dependency Parsing | Analyze sentence structure |
| Text Generation | Generate text based on a prompt |
| Conversational Dialogue | Engage in conversation |

### Channels

| Channel | Description |
| --- | --- |
| Web | Web interface for users |
| LINE | LINE messaging platform |
| Telegram | Telegram messaging platform |
| Discord | Discord messaging platform |
| Slack | Slack messaging platform |
| Facebook | Facebook messaging platform |

### Voice Integration

| Voice | Description |
| --- | --- |
| STT | Speech-to-text |
| TTS | Text-to-speech |

### Self-Hosted

nanobot can be self-hosted on your own infrastructure. It supports Docker and can be deployed to AWS Lambda.

### License

nanobot is licensed under the MIT License.

## Getting Started

To get started with nanobot, follow these steps:

1. Clone the repository: `git clone https://github.com/yukihamada/nanobot.git`
2. Build the binary: `cargo build --release`
3. Run the binary: `cargo run --release`
4. Configure the API keys: `nano config.json`
5. Start the server: `cargo run --release -- server`

## Contributing

Contributions are welcome! To contribute, follow these steps:

1. Fork the repository: `git fork https://github.com/yukihamada/nanobot.git`
2. Create a new branch: `git branch my-branch`
3. Make changes: `git add .`
4. Commit changes: `git commit -m "My changes"`
5. Push changes: `git push origin my-branch`
6. Create a pull request: `git pull-request`

## License

nanobot is licensed under the MIT License.

## Acknowledgments

* [chatweb.ai](https://chatweb.ai) for providing the initial use case and funding
* [teai.io](https://teai.io) for providing additional use cases and funding
* [Rust](https://www.rust-lang.org/) for providing a safe and efficient programming language
* [AWS Lambda](https://aws.amazon.com/lambda/) for providing a scalable and cost-effective deployment platform
