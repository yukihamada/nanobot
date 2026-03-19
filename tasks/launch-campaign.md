# teai.io Launch Campaign

## Twitter/X Post (Japanese)

### Main Announcement
```
teai.io をリリースしました

OpenAI互換のLLM APIゲートウェイ。
1つのAPIキーで50以上のモデルにアクセス。

- GPT-4o / Claude / Gemini / DeepSeek / Qwen3
- 日本語特化モデル Nemotron 9B は無料・無制限
- 既存のOpenAI SDKコードを1行変更するだけ
- 入力 ¥10/100万トークン〜

https://teai.io
```

### Technical Thread
```
1/ teai.io の技術的な特徴

OpenAI SDKそのまま使える互換API
base_url を変更するだけ:

from openai import OpenAI
client = OpenAI(
    base_url="https://api.teai.io/v1",
    api_key="te_xxx"
)

2/ 50以上のモデルを統一API経由で利用可能

GPT-4o, GPT-4.1, Claude Sonnet, Gemini 2.5 Flash/Pro,
DeepSeek V3/R1, Qwen3 235B/32B, Llama 4, Grok 3,
Nemotron, Mistral, Cohere, Phi-4 など

3/ 日本語特化

Nemotron 9B (日本語最適化) は無料・無制限
日本語品質を重視したモデル選定
数学・論理問題は内部で英語推論→日本語出力 (XLT)

4/ 料金は業界最安レベル

Free: 100クレジット + Nemotron無制限
Pro: ¥4,350/月 (300,000クレジット)

GPT-4o: 入力¥375/出力¥1,125 per 100万トークン
Claude Sonnet: 入力¥450/出力¥1,350
Gemini Flash: 入力¥10 (最安)

5/ OpenAI互換なので移行は簡単

pip install openai

export TEAI_API_KEY=te_xxx
# base_url を変えるだけ

ドキュメント: https://teai.io/docs
```

## Product Hunt

### Tagline
OpenAI-compatible API gateway for 50+ LLM models — Japanese-first

### Description
teai.io is an LLM API gateway that lets you access 50+ models (GPT-4o, Claude, Gemini, DeepSeek, Qwen3, Llama 4, and more) through a single OpenAI-compatible API key. Just change your base_url — no code rewrite needed.

Built for Japanese developers with Nemotron 9B (free, unlimited), cross-lingual reasoning (XLT), and pricing in JPY.

### Key Features
- 50+ models, one API key
- OpenAI SDK drop-in compatible
- Free tier with Nemotron 9B (unlimited)
- SSE streaming, tool calling support
- Japanese-first with XLT reasoning
- From ¥10/million tokens

## Hacker News

### Title
Show HN: teai.io – OpenAI-compatible gateway for 50+ LLM models

### Text
I built teai.io, an LLM API gateway that gives you access to 50+ models through a single OpenAI-compatible endpoint.

The idea: change `base_url` to `https://api.teai.io/v1` and use any model — GPT-4o, Claude, Gemini, DeepSeek, Qwen3, Llama 4, etc. — with your existing OpenAI SDK code.

Key differentiators from OpenRouter:
- Nemotron 9B (Japanese-optimized) is free and unlimited
- Built-in XLT reasoning (model thinks in English, responds in Japanese)
- Competitive pricing (Gemini Flash from ¥10/M tokens)
- Agentic mode with tool calling

Stack: Rust (axum) on AWS Lambda, DynamoDB, multi-provider failover with circuit breaker.

Free tier includes 100 credits. Try it: https://teai.io

## Reddit (r/LocalLLaMA, r/MachineLearning)

### Title
teai.io: OpenAI-compatible API gateway with 50+ models including free Nemotron 9B

### Body
Launched teai.io — an API gateway that gives you unified access to GPT-4o, Claude Sonnet, Gemini, DeepSeek, Qwen3, Llama 4, and 40+ more models through a standard OpenAI-compatible API.

What makes it different:
- Drop-in replacement: just change base_url, keep using OpenAI SDK
- Nemotron 9B (Japanese-optimized) is completely free, unlimited
- Built in Rust for low latency, runs on AWS Lambda
- Automatic failover between providers
- Streaming, tool calling, all OpenAI features supported

Free signup at https://teai.io, docs at https://teai.io/docs
