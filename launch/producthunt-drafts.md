# Product Hunt Launch Drafts

---

## chatweb.ai

**Name**: chatweb.ai

**Tagline**: Voice-first AI assistant for LINE, Telegram & Web

**Topics**: Artificial Intelligence, Productivity, Developer Tools, Chatbots

---

### Description

**Talk to AI your way -- by voice or text, on LINE, Telegram, or Web.**

chatweb.ai is a voice-first AI assistant designed for people who want to interact with AI naturally. Press the push-to-talk button, speak your question, and get a spoken response back. Or type -- it works both ways. It runs on LINE, Telegram, and the web, so you can start a conversation on LINE during your commute and pick it up on the web when you sit down at your desk.

**Multi-model, multi-tool, multi-channel.** Under the hood, chatweb.ai connects to 5+ AI models including Claude, GPT-4o, Gemini, and DeepSeek. It selects the best model for each channel automatically -- the smartest model on web, concise answers on LINE. Built-in tools let the AI search the web, generate code, check the weather, do calculations, and call MCP tools -- all within the chat flow. Long-term memory means the AI remembers your preferences and past conversations across sessions.

**Built for speed in Rust.** The entire platform runs on a single AWS Lambda function written in Rust. Response times average under 2 seconds. Cold starts are under 200ms. Uptime is 99.9%. Multi-channel sync means your conversations, credits, and memory follow you everywhere. Link your LINE, Telegram, and web accounts with the /link command or a QR code.

**Free to start.** Every account gets 1,000 credits per month for free -- no credit card required. Starter plan is $9/month, Pro is $29/month. Japanese-first but fully bilingual (JA/EN).

---

### Maker Comment

Hi PH! I built chatweb.ai because I wanted an AI assistant that works everywhere I do -- LINE for quick questions, Telegram for dev stuff, and Web for deep work. The voice feature was a personal itch -- I wanted to just talk to AI while cooking or commuting.

Built entirely in Rust for speed. The same Lambda function serves all channels with <2s response time. Free to try, no credit card needed. Would love your feedback!

-- @yukihamada (also maker of ElioChat)

---

### Images Needed

1. **Hero**: Screenshot of web chat interface with voice button highlighted
2. **LINE Chat**: Conversation on LINE showing the channel badge and concise responses
3. **Voice Demo**: GIF of push-to-talk flow -- press button, speak, see transcription, hear AI response
4. **Multi-Channel Sync**: Diagram showing conversation flowing between LINE, Telegram, and Web with shared memory
5. **Tool Usage**: Screenshot of web search results appearing inline in a chat conversation

---

### Launch Checklist

- [ ] Create Product Hunt maker profile
- [ ] Prepare 5 images/GIFs (listed above)
- [ ] Write first comment as maker
- [ ] Schedule launch for Tuesday 12:01 AM PT (best day for PH)
- [ ] Prepare social media posts for launch day
- [ ] Set up chatweb.ai/producthunt redirect for launch-day traffic
- [ ] Test free signup flow end-to-end

---
---

## teai.io

**Name**: teai.io

**Tagline**: AI agent platform for devs -- MCP tools, multi-model, REST API

**Topics**: Developer Tools, Artificial Intelligence, APIs, Open Source

---

### Description

**One API. Five models. Built-in tools. Zero hassle.**

teai.io is a developer-focused AI agent platform. Send a single API request and get responses from Claude, GPT-4o, Gemini, or DeepSeek -- the platform picks the best model or lets you choose. No need to manage multiple API keys, handle failover logic, or build tool integrations from scratch.

**MCP tools built in.** teai.io comes with integrated MCP tools: web search, file operations, shell execution, and browser automation. The AI agent can call these tools autonomously during a conversation -- search the web for current information, run code, fetch web pages, and return structured results. The tool-calling flow uses a three-phase approach (force tool use, then auto, then text generation) for reliable results.

**14+ channel integrations, one binary.** The entire platform runs on a single Rust binary deployed as an AWS Lambda function (ARM64). It serves the web UI, REST API, webhooks, OAuth flows, and integrations for LINE, Telegram, Discord, Slack, Microsoft Teams, and more. SSE streaming gives you real-time token-by-token responses. Cold starts are under 200ms. Average response time is under 2 seconds.

**Open source and free to start.** The source code is at github.com/yukihamada/nanobot. Free tier gives you 1,000 credits per month. Developer plan is $19/month. Team plan is $49/month. Try it right now: `curl -X POST https://teai.io/api/v1/chat -d '{"message":"Hello"}'`

---

### Maker Comment

Hey PH! teai.io started as a personal Rust project (nanobot) to unify all my AI tool usage into one API. I was tired of managing multiple API keys and switching between ChatGPT, Claude, and Gemini.

Now it's a full platform: one REST endpoint, 5+ models, MCP tools for web search and code execution, and 14 channel integrations. The whole thing runs on a single AWS Lambda function in Rust -- cold starts under 200ms.

Try it free:

```
curl -X POST https://teai.io/api/v1/chat \
  -H "Content-Type: application/json" \
  -d '{"message": "Hello"}'
```

Source code: github.com/yukihamada/nanobot -- stars appreciated!

-- @yukihamada

---

### Images Needed

1. **Hero**: teai.io landing page screenshot showing the main value proposition
2. **Terminal Demo**: Screenshot of curl command and JSON response in a terminal
3. **Features Grid**: Visual grid showing MCP tools (web search, shell, file ops, browser), multi-model support, channel integrations
4. **Code Generation**: GIF showing a code generation request and streaming response
5. **Architecture Diagram**: Rust + Lambda + API Gateway + DynamoDB + multi-model providers

---

### Launch Checklist

- [ ] Create teai.io landing page
- [ ] Prepare 5 images/GIFs (listed above)
- [ ] Write first comment as maker
- [ ] Schedule launch (different week from chatweb.ai -- space them 2+ weeks apart)
- [ ] Prepare social media posts
- [ ] Ensure API demo endpoint works without signup
- [ ] Test curl example end-to-end
- [ ] Add Product Hunt badge to teai.io landing page
