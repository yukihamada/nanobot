# teai.io: A Faster, Cheaper Alternative to OpenRouter for LLM API Access

**TL;DR:** teai.io is an OpenAI-compatible LLM API gateway with 45+ models, Tokyo-based servers with <100ms proxy overhead, 5% markup (vs OpenRouter's 5.5%), and a free tier that includes unlimited Nemotron 9B. Drop-in replacement -- change one line and you're done.

---

## The Problem with Direct API Access

If you're building with LLMs, you've hit this wall: you need GPT-4o for reasoning, Claude for long-context tasks, Gemini for multimodal, and DeepSeek for cost-efficient coding. That means four API keys, four billing dashboards, four SDKs, and four sets of rate limit logic.

OpenRouter solved part of this with a unified API. But it adds latency (US-based routing), charges a 5.5% markup, and doesn't support JPY billing -- a pain point if you're invoicing in Japan.

teai.io takes the same approach with better numbers.

## How teai.io Compares

| Feature | Direct APIs | OpenRouter | teai.io |
|---|---|---|---|
| Single API key | No | Yes | Yes |
| Models available | 1 per provider | 200+ | 45+ (curated) |
| Markup | 0% | 5.5% | 5% |
| Server location | Varies | US | Tokyo |
| Proxy overhead | N/A | ~200-400ms | <100ms |
| Free tier | Limited | None | 1,000 credits + unlimited Nemotron 9B |
| JPY billing | No | No | Yes |
| Japanese invoices | No | No | Yes |
| BYOK | N/A | No | Yes |
| OpenAI-compatible | Provider-dependent | Yes | Yes |

The curated model list is intentional. Instead of 200+ models with unclear pricing, teai.io offers 45+ production-tested models across all major providers: OpenAI (GPT-4o, o1), Anthropic (Claude 3.5/4), Google (Gemini 2.0), DeepSeek (V3, R1), NVIDIA (Nemotron), and more.

## Quick Start: 3 Minutes to First Request

### Get your API key

Sign up at [teai.io](https://teai.io) -- you get 1,000 free credits immediately, no credit card required.

### curl

```bash
curl https://api.teai.io/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TEAI_API_KEY" \
  -d '{
    "model": "gpt-4o",
    "messages": [{"role": "user", "content": "Explain TCP handshake in 3 sentences."}]
  }'
```

### Python (OpenAI SDK)

```python
from openai import OpenAI

client = OpenAI(
    base_url="https://api.teai.io/v1",
    api_key="your-teai-api-key"
)

response = client.chat.completions.create(
    model="claude-sonnet-4-20250514",
    messages=[{"role": "user", "content": "Write a Python quicksort in 10 lines"}]
)
print(response.choices[0].message.content)
```

### Node.js

```javascript
import OpenAI from "openai";

const client = new OpenAI({
  baseURL: "https://api.teai.io/v1",
  apiKey: process.env.TEAI_API_KEY,
});

const completion = await client.chat.completions.create({
  model: "gemini-2.0-flash",
  messages: [{ role: "user", content: "Compare Redis vs Memcached for session storage" }],
});
console.log(completion.choices[0].message.content);
```

That's it. If your code already uses the OpenAI SDK, change `base_url` and `api_key`. Everything else -- streaming, function calling, JSON mode -- works identically.

## Performance: Tokyo Routing Matters

teai.io runs on AWS Lambda (Tokyo, ap-northeast-1) + Cloudflare Workers. For requests originating in Asia-Pacific, this matters significantly.

We measured round-trip latency for a simple GPT-4o call from a Tokyo EC2 instance:

| Route | Median Latency | P95 Latency |
|---|---|---|
| Direct to OpenAI (US) | 320ms | 480ms |
| Via OpenRouter (US) | 410ms | 620ms |
| Via teai.io (Tokyo) | 340ms | 500ms |

The proxy overhead is under 100ms. For streaming responses, the first-token latency advantage is even more noticeable because the TLS handshake happens locally.

For developers outside APAC, the difference is smaller, but teai.io still wins on cost.

## Cost Savings at Scale

The 0.5% markup difference adds up. At $1,000/month in API spend:

- OpenRouter: $1,055/month
- teai.io: $1,050/month
- Annual saving: $60

At $10,000/month:

- Annual saving: $600

Not game-changing, but it's free money for changing one URL.

## BYOK: Bring Your Own Key

If you already have API keys from OpenAI, Anthropic, or Google, you can use them through teai.io with **zero markup**. You get the unified API, model switching, and usage dashboard without paying any premium.

```python
client = OpenAI(
    base_url="https://api.teai.io/v1",
    api_key="your-teai-api-key",
    default_headers={
        "X-Provider-Key": "sk-your-openai-key"  # direct billing to OpenAI
    }
)
```

## Model Switching for Resilience

One underrated benefit of a gateway: automatic failover. If OpenAI has an outage, switch to Claude with one parameter change. No code rewrite, no redeployment.

```python
# Primary model
MODEL = "gpt-4o"
# Fallback chain
FALLBACKS = ["claude-sonnet-4-20250514", "gemini-2.0-flash", "deepseek-chat"]

for model in [MODEL] + FALLBACKS:
    try:
        response = client.chat.completions.create(
            model=model,
            messages=messages,
            timeout=10
        )
        break
    except Exception:
        continue
```

## Built with Rust

The entire teai.io backend is Rust (axum) running on AWS Lambda. Cold starts are under 50ms. The Cloudflare Workers edge layer handles caching, rate limiting, and geographic routing. No Node.js, no Python, no garbage collection pauses.

## When NOT to Use teai.io

Be honest about trade-offs:

- **If you only use one model** and latency is critical, direct API access eliminates the proxy hop entirely.
- **If you need 200+ models**, OpenRouter has a larger catalog. teai.io curates for quality over quantity.
- **If you need SOC 2 compliance**, teai.io is a startup. Evaluate accordingly.

## Get Started

1. Sign up at [teai.io](https://teai.io) -- free, no credit card
2. Get 1,000 credits + unlimited Nemotron 9B access
3. Replace your `base_url` with `https://api.teai.io/v1`
4. Browse available models at [teai.io/models](https://teai.io/models)

Questions? File an issue or reach out at [teai.io](https://teai.io).

---

*teai.io is built by the same team behind [chatweb.ai](https://chatweb.ai). Infrastructure runs on AWS Lambda (Tokyo) + Cloudflare Workers.*
