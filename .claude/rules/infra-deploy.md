---
paths:
  - "infra/**/*"
  - "fly.toml"
  - "Dockerfile"
  - "workers/**/*"
---
# Infrastructure & Deploy Rules

## デプロイ先
| ターゲット | ドメイン | 設定ファイル |
|-----------|---------|------------|
| AWS Lambda (ap-northeast-1) | chatweb.ai, api.chatweb.ai | infra/template.yaml |
| Fly.io (nrt) | teai.io (teai-io app) | fly.toml, Dockerfile |
| Cloudflare Workers | エッジプロキシ | workers/teai-edge/wrangler.toml |
| Modal (GPU) | TTS, 音声分析 | infra/modal/*.py |

## Lambda デプロイ
```bash
# 高速デプロイ（推奨）
./infra/deploy-fast.sh

# 手動デプロイ
cd target/lambda/bootstrap && zip -j /tmp/nanobot-lambda.zip bootstrap
aws lambda update-function-code --function-name nanobot --zip-file "fileb:///tmp/nanobot-lambda.zip" --region ap-northeast-1
aws lambda publish-version --function-name nanobot --region ap-northeast-1
aws lambda update-alias --function-name nanobot --name live --function-version <VER> --region ap-northeast-1
```

## Fly.io
- app: teai-io, region: nrt (Tokyo)
- min_machines_running: 1 (always warm)
- memory: 512MB
- Dockerfile: binary name = chatweb, features = http-api,stripe

## Cloudflare Workers エッジプロキシ (workers/teai-edge/)
- PRIMARY_BACKEND: https://api.chatweb.ai (Lambda)
- FALLBACK_BACKEND: https://nanobot-ai.fly.dev (Fly.io)
- ヘルスチェック → 最適バックエンド選択

## Modal TTS
- infra/modal/kokoro_tts.py — Kokoro TTS (EN/JA/ZH)
- infra/modal/voice_analysis.py — 音声品質スコアリング
- `modal deploy` / `modal run` でデプロイ

## 環境変数（重要なもの）
MODAL_TTS_URL, STRIPE_SECRET_KEY, STRIPE_WEBHOOK_SECRET, STRIPE_PRICE_*,
ELEVENLABS_API_KEY, ADMIN_SESSION_KEYS, PASSWORD_HMAC_KEY,
LOCAL_MODEL_URL, LOCAL_TOKENIZER_URL, META_CONVERSIONS_API_TOKEN, META_PIXEL_ID
