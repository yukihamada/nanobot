# CLAUDE.md — nanobot / chatweb.ai

## プロジェクト概要
chatweb.ai — 日本発、音声中心のマルチチャネルAIアシスタント
日本を愛し、人を愛し、みんなに勇気と元気と幸せをもたらすAGI。
Rust (axum) + AWS Lambda + DynamoDB + API Gateway

## ビジョン
- **Voice-first**: 音声で話しかけ、音声で即応答。Push-to-talk体験。
- **日本語中心**: 日本のユーザーを最優先。LINE・Telegram連携。
- **爆速**: SSEストリーミング、並列ツール実行、最賢モデル自動選択。
- **長期記憶**: OpenClaw風のデイリーログ + 長期記憶をDynamoDBで管理。
- **マルチチャネル**: Web, LINE, Telegram, Facebook, Discord, Slack, Teams

## アーキテクチャ
```
ユーザー → API Gateway (chatweb.ai / api.chatweb.ai)
         → AWS Lambda (nanobot, ARM64, ap-northeast-1)
         → DynamoDB (セッション、ユーザー、設定、メモリ)
         → LLM (Anthropic / OpenAI / Google) ← LoadBalancedProvider
```

## ディレクトリ構成
```
crates/nanobot-core/src/
  service/http.rs       — メインHTTPハンドラー（チャット、認証、課金、TTS、メモリ等）
  service/integrations.rs — ツール統合（web_search, calculator等）
  service/auth.rs       — 認証・クレジット計算
  channel/line.rs       — LINE Webhook
  channel/telegram.rs   — Telegram Webhook
  channel/facebook.rs   — Facebook Messenger Webhook
  provider/              — LLMプロバイダー（OpenAI, Anthropic, Google）
  memory/               — 長期記憶バックエンド（DynamoDB / ファイル）
web/
  index.html            — メインWebフロントエンド（SPA、全HTMLインライン）
  pricing.html          — 料金ページ（クーポンUI付き）
infra/
  template.yaml         — SAMテンプレート
  deploy.sh             — デプロイスクリプト
```

## UI設計方針
- **Voice-centric**: 空状態では大きなマイクボタンが中央に表示
- **Auto agent**: エージェント選択は非表示（自動選択）
- **Auto-TTS**: 音声入力時は応答を自動で音声再生
- **TTS caching**: 同じテキストは再フェッチせずキャッシュから再生
- **チャネル連携**: 確認ダイアログ → QRコードモーダル
- **地域別チャネル順序**: 日本=LINE,Telegram優先 / 海外=WhatsApp,Telegram優先
- **スマホ最適化**: safe-area-inset対応、タッチターゲット38px以上

## セキュリティ
- **Admin鍵**: `ADMIN_SESSION_KEYS`環境変数（メール対応: yuki@hamada.tokyo, mail@yukihamada.jp）
- **CORS**: chatweb.ai, api.chatweb.ai, localhost:3000のみ許可
- **レート制限**: DynamoDB atomic counter (login 5/min, register 3/min)
- **監査ログ**: AUDIT#{date}レコード、90日TTL
- **入力検証**: body 1MB, message 32K, email 254, password 8-128
- **Telegram webhook検証**: X-Telegram-Bot-Api-Secret-Token
- **パスワードHMAC**: PASSWORD_HMAC_KEY優先、GOOGLE_CLIENT_SECRETフォールバック

## 長期記憶（OpenClaw inspired）
- **DynamoDB PKパターン**: `MEMORY#{user_id}` + sk: `LONG_TERM` or `DAILY#{date}`
- **読み込み**: チャット開始時にsystem promptに注入
- **書き込み**: 各会話終了後にデイリーログへ自動追記（fire-and-forget）
- **コンテキスト**: 長期記憶 + 今日のメモ + セッション履歴20件

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
- DynamoDB PKパターン: `USER#{id}`, `AUTH#{token}`, `USAGE#{id}#{date}`, `MEMORY#{id}`
- `deduct_credits()` は差し引いたクレジット数を返す。残高は別途 `get_or_create_user()` で取得

### チャネル別動作
| チャネル | モデル | プロンプト特性 |
|---------|--------|--------------|
| Web | claude-sonnet-4-5 (自動) | 詳細・ツール使用可・音声中心 |
| LINE | デフォルト(高速) | 200字以内、絵文字、箇条書き |
| Telegram | デフォルト(高速) | 300字以内、Markdown記法 |
| Facebook | デフォルト(高速) | 300字以内、簡潔 |

### API エンドポイント
- `POST /api/v1/chat` — メインチャット（credits_used/credits_remaining返却）
- `POST /api/v1/chat/stream` — SSEストリーミング応答
- `POST /api/v1/speech/synthesize` — TTS（OpenAI tts-1, nova）
- `GET /api/v1/auth/me` — 認証確認 + クレジット情報
- `POST /webhooks/line` — LINE Webhook
- `POST /webhooks/telegram` — Telegram Webhook
- `GET/POST /webhooks/facebook` — Facebook Messenger Webhook
- `POST /webhooks/stripe` — Stripe Webhook

### フロントエンド（web/index.html）
- 全てインラインSPA（CSS + HTML + JS が1ファイル）
- **Voice-first**: 大きなマイクボタンが主要CTA、Push-to-talk体験
- STT: Web Speech API (ja-JP)、マイクボタン、自動送信
- TTS: /api/v1/speech/synthesize → MP3再生 + キャッシュ
- Auto-TTS: 音声入力→応答を自動で音声再生
- SSE: /api/v1/chat/stream でリアルタイム応答
- クレジット表示: サイドバー、応答ごとにリアルタイム更新
- appAddMsg() でbotメッセージにTTSボタン自動付与

### CI/CD
- `main`へのpushで自動デプロイ（test → build → canary → production）
- PRにはCIテスト通過 + 1 approving review が必要
- `enforce_admins: false` → オーナーはバイパス可能

### 既知の問題
- `integrations.rs` の `base64` クレートが未追加（コンパイルエラー、http.rsとは無関係）
- Web Speech APIはChrome/Edgeのみ対応、Firefox/Safariは自動非表示
