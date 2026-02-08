# CLAUDE.md — nanobot / chatweb.ai

## プロジェクト概要
chatweb.ai — マルチチャネルAIチャットサービス（Web, LINE, Telegram）
Rust (axum) + AWS Lambda + DynamoDB + API Gateway

## アーキテクチャ
```
ユーザー → API Gateway (chatweb.ai / api.chatweb.ai)
         → AWS Lambda (nanobot, ARM64, ap-northeast-1)
         → DynamoDB (セッション、ユーザー、設定)
         → LLM (Anthropic / OpenAI / Google) ← LoadBalancedProvider
```

## ディレクトリ構成
```
crates/nanobot-core/src/
  service/http.rs       — メインHTTPハンドラー（チャット、認証、課金、TTS等）
  service/integrations.rs — ツール統合（web_search, calculator等）
  service/auth.rs       — 認証・クレジット計算
  channel/line.rs       — LINE Webhook
  channel/telegram.rs   — Telegram Webhook
  provider/              — LLMプロバイダー（OpenAI, Anthropic, Google）
web/
  index.html            — メインWebフロントエンド（SPA、全HTMLインライン）
infra/
  template.yaml         — SAMテンプレート
  deploy.sh             — デプロイスクリプト
```

## 主要な制約・注意事項

### ビルド
- `include_str!()` でHTMLをバイナリに埋め込み → **HTML変更後は必ずリビルド**
- クロスコンパイル:
  ```
  RUSTUP_TOOLCHAIN=stable RUSTC=/Users/yuki/.rustup/toolchains/stable-aarch64-apple-darwin/bin/rustc \
    cargo zigbuild --manifest-path crates/nanobot-lambda/Cargo.toml --release --target aarch64-unknown-linux-gnu
  ```

### コード規約
- `ChatResponse`を変更する場合、全ての`Json(ChatResponse {...})`箇所を更新すること
- CORS: chatweb.aiとapi.chatweb.aiは同一Lambdaから配信 → 相対URLを使用
- DynamoDB PKパターン: `USER#{id}`, `AUTH#{token}`, `USAGE#{id}#{date}`
- `deduct_credits()` は差し引いたクレジット数を返す。残高は別途 `get_or_create_user()` で取得

### チャネル別動作
| チャネル | モデル | プロンプト特性 |
|---------|--------|--------------|
| Web | claude-sonnet-4-5 (自動) | 詳細・ツール使用可 |
| LINE | デフォルト(高速) | 200字以内、絵文字、箇条書き |
| Telegram | デフォルト(高速) | 300字以内、Markdown記法 |

### API エンドポイント
- `POST /api/v1/chat` — メインチャット（credits_used/credits_remaining返却）
- `POST /api/v1/speech/synthesize` — TTS（OpenAI tts-1, nova）
- `GET /api/v1/auth/me` — 認証確認 + クレジット情報
- `POST /webhooks/line` — LINE Webhook
- `POST /webhooks/telegram` — Telegram Webhook
- `POST /webhooks/stripe` — Stripe Webhook

### フロントエンド（web/index.html）
- 全てインラインSPA（CSS + HTML + JS が1ファイル）
- STT: Web Speech API (ja-JP)、マイクボタン、自動送信
- TTS: /api/v1/speech/synthesize → MP3再生
- クレジット表示: サイドバー、応答ごとにリアルタイム更新
- appAddMsg() でbotメッセージにTTSボタン自動付与

### CI/CD
- `main`へのpushで自動デプロイ（test → build → canary → production）
- PRにはCIテスト通過 + 1 approving review が必要
- `enforce_admins: false` → オーナーはバイパス可能

### 既知の問題
- `integrations.rs` の `base64` クレートが未追加（コンパイルエラー、http.rsとは無関係）
- Web Speech APIはChrome/Edgeのみ対応、Firefox/Safariは自動非表示
