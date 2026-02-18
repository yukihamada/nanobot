# ElioChat API Integration

ElioChat (local LLM app) can use chatweb.ai / teai.io APIs for enhanced capabilities.

## Quick Start

### 1. Get API Key

1. Visit https://chatweb.ai or https://teai.io
2. Sign up (chatweb.ai: 100 credits, teai.io: 1000 credits)
3. Go to Settings → API Keys
4. Create new API key (starts with `cw_`)

### 2. Configure ElioChat

Set API endpoint and key in ElioChat settings:

```
API Endpoint: https://api.chatweb.ai/api/v1/chat
API Key: cw_YOUR_KEY_HERE
```

Or for teai.io:

```
API Endpoint: https://api.teai.io/api/v1/chat
API Key: cw_YOUR_KEY_HERE
```

### 3. Make Requests

#### Basic Chat

```bash
curl -X POST https://api.chatweb.ai/api/v1/chat \
  -H "Authorization: Bearer cw_YOUR_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "message": "Hello!",
    "session_id": "eliochat_session_123"
  }'
```

#### Streaming (SSE)

```bash
curl -X POST https://api.chatweb.ai/api/v1/chat/stream \
  -H "Authorization: Bearer cw_YOUR_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "message": "Hello!",
    "session_id": "eliochat_session_123"
  }'
```

## API Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/v1/chat` | POST | Synchronous chat |
| `/api/v1/chat/stream` | POST | Streaming chat (SSE) |
| `/api/v1/auth/me` | GET | Get user info + credits |
| `/api/v1/apikeys` | GET | List API keys |
| `/api/v1/apikeys` | POST | Create API key |
| `/api/v1/apikeys/{id}` | DELETE | Delete API key |

## Request Format

### Chat Request

```json
{
  "message": "Your message here",
  "session_id": "unique_session_id",
  "model": "openai/gpt-4o",  // optional
  "temperature": 0.7,         // optional
  "max_tokens": 2000          // optional
}
```

### Response Format

```json
{
  "response": "AI response here",
  "session_id": "unique_session_id",
  "agent": "assistant",
  "tools_used": ["web_search"],
  "credits_used": 37,
  "credits_remaining": 963,
  "model_used": "openai/gpt-4o",
  "input_tokens": 150,
  "output_tokens": 45
}
```

## Available Models

| Model | ID | Credits/1k tokens |
|-------|-----|-------------------|
| GPT-4o | `openai/gpt-4o` | 15 input / 60 output |
| GPT-4o-mini | `openai/gpt-4o-mini` | 0.5 input / 1.5 output |
| Claude Sonnet 4.5 | `anthropic/claude-sonnet-4-5` | 9 input / 45 output |
| Gemini Flash | `gemini-2.0-flash-lite` | 0.25 input / 0.5 output |

## Available Tools

- `web_search` - Brave Search API
- `web_fetch` - Fetch and parse web pages
- `calculator` - Math calculations
- `weather` - Weather data
- `translate` - Language translation
- `wikipedia` - Wikipedia search
- `date_time` - Current date/time
- `qr_code` - QR code generation
- `news_search` - News search

### Agentic Tools (Starter+ plans)

- `code_execute` - Execute shell/python/nodejs code
- `file_read` - Read files from sandbox
- `file_write` - Write files to sandbox
- `file_list` - List files in sandbox

## Credits & Pricing

### chatweb.ai
- Free: 100 initial credits
- Starter: $19/mo (25,000 credits)
- Pro: $49/mo (300,000 credits)

### teai.io
- Free: 1000 initial credits
- Then charge as you need
- Same pricing as chatweb.ai

### Credit Calculation

Credits = (input_tokens / 1000) * input_rate + (output_tokens / 1000) * output_rate

Example: GPT-4o with 150 input + 45 output tokens
= (150/1000)*15 + (45/1000)*60 = 2.25 + 2.7 = ~5 credits

## Error Handling

### Common Errors

| Status | Error | Solution |
|--------|-------|----------|
| 401 | Unauthorized | Check API key format (`cw_...`) |
| 402 | Insufficient credits | Top up credits |
| 429 | Rate limit exceeded | Wait or upgrade plan |
| 500 | Server error | Retry with exponential backoff |

### Example Error Response

```json
{
  "error": "Insufficient credits",
  "credits_remaining": 0,
  "plan": "free"
}
```

## CORS & Security

- API keys are **secret** - never expose in client-side code
- For web apps, use server-side proxy
- For desktop/mobile apps, API keys are OK
- Localhost is allowed in dev mode only

## Rate Limits

| Plan | Requests/min |
|------|--------------|
| Free | 5 |
| Starter | 30 |
| Pro | 120 |
| Enterprise | 600 |

## Support

- Docs: https://chatweb.ai/docs
- API Status: https://status.chatweb.ai
- Issues: https://github.com/anthropics/chatweb/issues
- Email: support@chatweb.ai

## Example: ElioChat Integration

```python
import requests

API_KEY = "cw_YOUR_KEY"
API_URL = "https://api.chatweb.ai/api/v1/chat"

def chat(message, session_id="eliochat_default"):
    headers = {
        "Authorization": f"Bearer {API_KEY}",
        "Content-Type": "application/json"
    }
    data = {
        "message": message,
        "session_id": session_id
    }

    response = requests.post(API_URL, headers=headers, json=data)

    if response.status_code == 200:
        result = response.json()
        print(f"AI: {result['response']}")
        print(f"Credits used: {result['credits_used']}")
        print(f"Credits remaining: {result['credits_remaining']}")
    else:
        print(f"Error: {response.json()}")

# Usage
chat("Hello, how are you?")
```

## Migration from Other Providers

### From OpenAI

Replace:
```python
openai.ChatCompletion.create(
    model="gpt-4o",
    messages=[{"role": "user", "content": "Hello"}]
)
```

With:
```python
requests.post(
    "https://api.chatweb.ai/api/v1/chat",
    headers={"Authorization": "Bearer cw_YOUR_KEY"},
    json={"message": "Hello", "session_id": "session_1"}
)
```

### Benefits over Direct OpenAI API

1. **Built-in tools**: Web search, calculator, etc.
2. **Multi-model**: Switch between providers easily
3. **Session management**: Automatic conversation history
4. **Cost tracking**: Per-request credit usage
5. **No API key juggling**: One key for multiple providers
