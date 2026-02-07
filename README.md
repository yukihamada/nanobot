# nanobot

AIエージェントプラットフォーム — LINE・TelegramにAIボットを数分でデプロイ。

**サイト:** [https://chatweb.ai](https://chatweb.ai)
**API:** [https://api.chatweb.ai](https://api.chatweb.ai)

## 特徴

- **マルチモデル** — GPT-4o, Claude, Gemini を切り替え可能
- **マルチチャネル** — LINE, Telegram, REST API
- **サーバーレス** — Rust製、AWS Lambda / Fly.io 対応
- **会話メモリ** — セッションを跨いで文脈を記憶
- **無料プラン** — 登録不要ですぐに使える

## クイックスタート

```bash
# APIでチャット
curl -X POST https://api.chatweb.ai/api/v1/chat \
  -H "Content-Type: application/json" \
  -d '{"message": "こんにちは"}'
```

## アーキテクチャ

```
LINE / Telegram / Web
        │
   API Gateway or Fly.io
        │
   Rust バイナリ (ARM64)
        │
   DynamoDB (セッション・メモリ)
```

## デプロイ

### AWS Lambda

```bash
brew install aws-sam-cli zig
cargo install cargo-zigbuild
rustup target add aarch64-unknown-linux-gnu

./infra/deploy.sh           # ビルド＆デプロイ
./infra/setup-webhook.sh    # Webhook設定
```

### Fly.io

```bash
fly launch --no-deploy
fly deploy
```

## 開発

```bash
cargo test -p nanobot-core  # テスト実行
cargo run                   # ローカルサーバー (:3000)
```

## ディレクトリ構成

```
crates/
  nanobot-core/    # コアライブラリ（チャネル, AI, セッション）
  nanobot-lambda/  # AWS Lambda ハンドラー
infra/             # SAMテンプレート, デプロイスクリプト
web/               # フロントエンド (SPA)
src/               # ローカルサーバー
```

## ライセンス

MIT
