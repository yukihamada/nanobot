# Hacker News — Show HN Post

---

## Title

Show HN: teai.io -- Multi-model AI agent built in Rust with MCP tools

---

## Post Text

I built teai.io, an AI agent platform that lets you access Claude, GPT-4o, Gemini, and DeepSeek through a single REST API.

Key features:

- One API, multiple models (auto-select or specify)
- Built-in MCP tools: web search, file ops, shell execution, browser automation
- SSE streaming for real-time responses
- 14+ channel integrations (Web, LINE, Telegram, Discord, Slack, Teams)
- Long-term memory across sessions
- <2s average response time

Tech stack: Rust (axum), AWS Lambda (ARM64), DynamoDB, API Gateway. The entire platform runs on a single Lambda function -- cold starts under 200ms.

Try it: https://teai.io (free, no signup required for demo)

API example:

```
curl -X POST https://teai.io/api/v1/chat \
  -H "Content-Type: application/json" \
  -d '{"message": "Write a Rust function for binary search"}'
```

Source: https://github.com/yukihamada/nanobot

I'm particularly interested in feedback on:

1. The MCP tool integration approach
2. Multi-model routing (how we select the best model per query)
3. The single-Lambda architecture for a full AI platform

Happy to answer any questions about the Rust/Lambda implementation!

---

## Anticipated HN Questions & Prepared Answers

### "Why not just use OpenRouter / LiteLLM?"

Good question. OpenRouter and LiteLLM solve the multi-model routing problem, but teai.io goes further: it's a full agent platform with built-in tool calling (MCP), long-term memory, and 14 channel integrations. You get an AI agent, not just a model proxy. Also, it's a single Rust binary -- no Python runtime, no Docker, no orchestration layer.

### "Why Rust for an AI platform?"

Two reasons: Lambda cold starts and memory efficiency. Rust on ARM64 Lambda gives us <200ms cold starts versus 2-5 seconds for Python. Memory usage stays under 128MB for most requests. For a platform that handles bursty traffic across 14 channels, this makes a real cost difference.

### "How do you handle web search from Lambda? Cloud IPs get blocked."

This was one of the hardest problems. Google, Bing, DuckDuckGo, and Amazon all block or CAPTCHA cloud IP ranges. We solved it with a two-step approach: use a search API for initial results, then fetch individual pages through Jina Reader (r.jina.ai), which handles JavaScript rendering and bypasses cloud IP blocks.

### "Single Lambda for everything -- doesn't that violate microservices best practices?"

It does, and intentionally. For a small team (solo developer), a single binary is far easier to deploy, debug, and reason about. The Lambda handles routing internally via axum, which is essentially the same as running a web server -- just invoked per-request. If I need to split it later, axum makes that straightforward since each route handler is already an independent function.

### "How does the MCP tool integration work?"

The chat flow uses a three-phase tool-calling approach:

1. First LLM call: `tool_choice = "required"` -- forces the model to call a tool if available
2. Second LLM call: `tool_choice = "auto"` -- model decides whether to call more tools based on results
3. Final LLM call: tools set to `None` -- forces text generation for the final response

This prevents the model from skipping tools when they'd be useful, while avoiding infinite tool-calling loops.

### "What's the DynamoDB async Rust gotcha you mentioned?"

In Tokio's async runtime, you can't call `block_on` from within an async context (it panics). When we needed to call DynamoDB synchronously from certain contexts, `std::thread::scope` didn't help because the closure runs on the current thread (still inside Tokio). The fix was `std::thread::spawn` to create a genuinely new thread that's outside the Tokio runtime, then `block_on` from there.

---

## Posting Strategy

- **Best time**: Weekday morning US time (Monday-Thursday, 8-10 AM ET)
- **Follow up**: Monitor comments for the first 2-3 hours, respond quickly
- **Tone**: Technical, humble, specific. HN respects honesty about limitations.
- **Don't**: Over-promote, use marketing language, compare to competitors negatively
- **Do**: Share specific technical details, acknowledge trade-offs, link to source code
