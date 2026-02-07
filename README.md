# nanobot

AIエージェントプラットフォーム — Web・LINE・Telegramで使えるAIアシスタントを数分でデプロイ。

**サイト:** [https://chatweb.ai](https://chatweb.ai)
**API:** [https://api.chatweb.ai](https://api.chatweb.ai)

## 特徴

- **マルチモデル** — GPT-4o, Claude Sonnet/Opus, Gemini を切り替え可能
- **マルチチャネル** — Web, LINE, Telegram をリアルタイム同期
- **チャネル同期** — `/link` コマンドで全チャネルの会話を統合
- **サーバーレス** — Rust製、AWS Lambda (ARM64) で高速・低コスト
- **会話メモリ** — DynamoDBでセッションを永続化、チャネル横断で文脈を記憶
- **フリーミアム** — 無料プランは登録不要、クーポンコード対応
- **QRコード** — リンクコード生成時にQR + ワンタップディープリンク

## すぐに試す

### Web
[https://chatweb.ai](https://chatweb.ai) にアクセスしてチャット開始

### LINE
友だち追加: [@619jcqqh](https://line.me/R/ti/p/@619jcqqh)

### Telegram
ボット: [@chatweb_ai_bot](https://t.me/chatweb_ai_bot)

### API
```bash
curl -X POST https://chatweb.ai/api/v1/chat \
  -H "Content-Type: application/json" \
  -d '{"message": "Hello!", "session_id": "my-session"}'
```

## チャネル同期

どのチャネルでも同じ会話を継続できます:

1. 任意のチャネルで `/link` と送信 → 6桁コード生成
2. 別チャネルで `/link CODE` と送信 → 会話が統合
3. 以降、どこからでも同じ会話の続きを

## アーキテクチャ

```
Web / LINE / Telegram
        |
   API Gateway (chatweb.ai)
        |
   AWS Lambda (Rust ARM64)
        |
   +----+----+
   |         |
DynamoDB   DynamoDB
(sessions) (config/links)
```

## 料金

| プラン | 価格 | 内容 |
|--------|------|------|
| Free | $0/月 | 1,000クレジット, GPT-4o-mini, Gemini Flash |
| Starter | $9/月 | 25,000クレジット, + GPT-4o, Claude Sonnet |
| Pro | $29/月 | 300,000クレジット, + Claude Opus, 全モデル |

クーポンコード `LAUNCH2026` で Starter プラン初月無料

## デプロイ

### ワンクリックデプロイ

| プラットフォーム | コマンド / リンク |
|---------|-----------|
| **Railway** | [![Deploy on Railway](https://railway.app/button.svg)](https://railway.app/template/nanobot) |
| **Render** | [![Deploy to Render](https://render.com/images/deploy-to-render-button.svg)](https://render.com/deploy?repo=https://github.com/yukihamada/nanobot) |
| **Koyeb** | [![Deploy to Koyeb](https://www.koyeb.com/static/images/deploy/button.svg)](https://app.koyeb.com/deploy?type=git&repository=yukihamada/nanobot) |
| **Docker** | `docker run -p 3000:3000 ghcr.io/yukihamada/nanobot` |
| **RunPod** | `runpodctl deploy --gpu 0 --image ghcr.io/yukihamada/nanobot` |

### AWS Lambda

```bash
brew install zig
cargo install cargo-zigbuild
rustup target add aarch64-unknown-linux-gnu

# ビルド
RUSTUP_TOOLCHAIN=stable cargo zigbuild \
  --manifest-path crates/nanobot-lambda/Cargo.toml \
  --release --target aarch64-unknown-linux-gnu

# デプロイ
cp target/aarch64-unknown-linux-gnu/release/bootstrap ./bootstrap
zip -j lambda.zip bootstrap
aws lambda update-function-code --function-name nanobot --zip-file fileb://lambda.zip
```

### Fly.io

```bash
fly launch --no-deploy
fly deploy
```

### ローカル開発

```bash
cargo run -- gateway --http --http-port 3000
# http://localhost:3000 でアクセス
```

## インフラ構成

| コンポーネント | サービス | リージョン | 冗長化 |
|------------|---------|---------|--------|
| Compute | AWS Lambda (ARM64 Graviton2) | ap-northeast-1 (東京) | 自動スケーリング (1,000+ 同時実行) |
| Database | Amazon DynamoDB | ap-northeast-1 (東京) | 3-AZ レプリケーション |
| API Gateway | Amazon API Gateway | ap-northeast-1 (東京) | Multi-AZ, 自動フェイルオーバー |
| Backup Compute | Fly.io | nrt (成田) | Warm standby |
| DNS / CDN | Cloudflare | Global Edge | グローバル分散 |
| CI/CD | GitHub Actions | - | Test → Build → Canary 10% → Production 100% |
| 決済 | Stripe | - | PCI DSS準拠 |

## 開発

```bash
cargo test --all        # テスト実行
cargo build             # デフォルトビルド
cargo build --features saas  # SaaS機能付きビルド
```

## ディレクトリ構成

```
crates/
  nanobot-core/    # コアライブラリ（チャネル, AI, セッション, HTTP API）
  nanobot-lambda/  # AWS Lambda ハンドラー
infra/             # SAMテンプレート, デプロイスクリプト
web/               # フロントエンド (index.html, pricing.html, etc.)
src/               # ローカルサーバー (CLI)
docs/              # プレスリリース等
```

## API エンドポイント

| Method | Path | 説明 |
|--------|------|------|
| POST | `/api/v1/chat` | AIチャット |
| GET | `/api/v1/sessions/{id}` | セッション取得（リンク解決対応） |
| GET | `/api/v1/sessions` | セッション一覧 |
| DELETE | `/api/v1/sessions/{id}` | セッション削除 |
| POST | `/api/v1/coupon/validate` | クーポンコード検証 |
| POST | `/webhooks/line` | LINE Webhook |
| POST | `/webhooks/telegram` | Telegram Webhook |
| POST | `/webhooks/stripe` | Stripe Webhook |
| GET | `/health` | ヘルスチェック |

## ライセンス

MIT
