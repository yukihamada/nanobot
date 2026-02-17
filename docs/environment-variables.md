# Environment Variables

Complete list of environment variables for configuring nanobot.

## LLM Providers

### Required (at least one)

| Variable | Description |
|----------|-------------|
| `OPENAI_API_KEY` | OpenAI API key (sk-...) |
| `ANTHROPIC_API_KEY` | Anthropic API key (sk-ant-...) |
| `GEMINI_API_KEY` | Google Gemini API key |

### Optional

| Variable | Description |
|----------|-------------|
| `GROQ_API_KEY` | Groq inference API key |
| `DEEPSEEK_API_KEY` | DeepSeek API key |
| `KIMI_API_KEY` | Kimi / Moonshot API key |
| `OPENROUTER_API_KEY` | OpenRouter (multi-model fallback) |

## Database

| Variable | Required | Description |
|----------|----------|-------------|
| `DYNAMODB_TABLE` | Yes (Lambda) | DynamoDB table name |
| `AWS_REGION` | No | AWS region (default: ap-northeast-1) |

## Application

| Variable | Required | Description | Default |
|----------|----------|-------------|---------|
| `BASE_URL` | No | Base URL for callbacks | https://chatweb.ai |
| `PORT` | No | HTTP port (local mode) | 3000 |

## Authentication

| Variable | Required | Description |
|----------|----------|-------------|
| `PASSWORD_HMAC_KEY` | Recommended | HMAC key for password hashing (32+ chars) |
| `ADMIN_SESSION_KEYS` | No | Comma-separated admin session keys |
| `GOOGLE_CLIENT_ID` | No | Google OAuth client ID |
| `GOOGLE_CLIENT_SECRET` | No | Google OAuth client secret |

## Channel Integrations

### LINE

| Variable | Required | Description |
|----------|----------|-------------|
| `LINE_CHANNEL_SECRET` | Yes | LINE channel secret |
| `LINE_CHANNEL_ACCESS_TOKEN` | Yes | LINE channel access token |

### Telegram

| Variable | Required | Description |
|----------|----------|-------------|
| `TELEGRAM_BOT_TOKEN` | Yes | Telegram bot token |
| `TELEGRAM_WEBHOOK_SECRET` | Recommended | X-Telegram-Bot-Api-Secret-Token |

### Facebook

| Variable | Required | Description |
|----------|----------|-------------|
| `FACEBOOK_PAGE_ACCESS_TOKEN` | Yes | Facebook page access token |
| `FACEBOOK_VERIFY_TOKEN` | Yes | Facebook webhook verify token |

### WhatsApp

| Variable | Required | Description |
|----------|----------|-------------|
| `WHATSAPP_TOKEN` | Yes | WhatsApp Cloud API token |
| `WHATSAPP_PHONE_ID` | Yes | WhatsApp phone number ID |

### Discord

| Variable | Required | Description |
|----------|----------|-------------|
| `DISCORD_BOT_TOKEN` | Yes | Discord bot token |
| `DISCORD_CLIENT_ID` | Yes | Discord client ID |

### Slack

| Variable | Required | Description |
|----------|----------|-------------|
| `SLACK_BOT_TOKEN` | Yes | Slack bot token (xoxb-...) |
| `SLACK_SIGNING_SECRET` | Yes | Slack signing secret |

## Billing

| Variable | Required | Description |
|----------|----------|-------------|
| `STRIPE_SECRET_KEY` | No | Stripe secret key (sk_live_...) |
| `STRIPE_WEBHOOK_SECRET` | No | Stripe webhook signing secret |
| `STRIPE_PRICE_STARTER` | No | Stripe price ID for Starter plan |
| `STRIPE_PRICE_PRO` | No | Stripe price ID for Pro plan |

## Tools & Integrations

| Variable | Required | Description |
|----------|----------|-------------|
| `BRAVE_API_KEY` | No | Brave Search API key |
| `JINA_API_KEY` | No | Jina Reader API key |
| `ELEVENLABS_API_KEY` | No | ElevenLabs TTS API key |
| `SUNO_API_KEY` | No | Suno music generation API key |
| `KLING_API_KEY` | No | Kling video generation API key |

## Local LLM (Fallback)

| Variable | Required | Description |
|----------|----------|-------------|
| `LOCAL_MODEL_URL` | No | URL to local GGUF model file |
| `LOCAL_TOKENIZER_URL` | No | URL to tokenizer config |

## Example .env File

```bash
# LLM Providers (at least one required)
OPENAI_API_KEY=sk-proj-...
ANTHROPIC_API_KEY=sk-ant-...
GEMINI_API_KEY=...

# Database
DYNAMODB_TABLE=nanobot-table

# Application
BASE_URL=https://chatweb.ai

# Authentication
PASSWORD_HMAC_KEY=$(openssl rand -hex 32)
ADMIN_SESSION_KEYS=admin-$(openssl rand -hex 16)

# Google OAuth
GOOGLE_CLIENT_ID=...apps.googleusercontent.com
GOOGLE_CLIENT_SECRET=...

# LINE
LINE_CHANNEL_SECRET=...
LINE_CHANNEL_ACCESS_TOKEN=...

# Telegram
TELEGRAM_BOT_TOKEN=...
TELEGRAM_WEBHOOK_SECRET=...

# Stripe (optional)
STRIPE_SECRET_KEY=sk_live_...
STRIPE_WEBHOOK_SECRET=whsec_...
```

## Generating Secure Secrets

```bash
# Password HMAC key (32 bytes = 64 hex chars)
openssl rand -hex 32

# Admin session key (16 bytes = 32 hex chars)
openssl rand -hex 16

# Telegram webhook secret (any random string)
openssl rand -base64 32
```

## AWS Secrets Manager (Recommended for Lambda)

```bash
# Create secret
aws secretsmanager create-secret \
  --name nanobot/api-keys \
  --secret-string '{
    "OPENAI_API_KEY": "sk-...",
    "ANTHROPIC_API_KEY": "sk-ant-...",
    "PASSWORD_HMAC_KEY": "..."
  }'

# Grant Lambda access
aws lambda add-permission \
  --function-name nanobot \
  --statement-id SecretsManagerAccess \
  --action secretsmanager:GetSecretValue \
  --principal secretsmanager.amazonaws.com
```

---

**Need help?** Check [deployment.md](deployment.md) for platform-specific instructions.
