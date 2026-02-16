---
paths:
  - "crates/**/*.rs"
---
# Rust Backend Rules

## HTTP Handler Module (service/http/)
- ルーター定義: `http/mod.rs` — 新エンドポイント追加時はここにルート登録
- ハンドラー: `http/handlers/` 配下にドメイン別分割（auth, billing, chat, conversations, voice, webhooks, pages, misc）
- 新ハンドラー追加時: 適切な handlers/*.rs に追加し、handlers/mod.rs で re-export

## Agentic Mode (chat.rs)
- ループ: `while current.has_tool_calls() && iteration < max_iterations`
- 各イテレーションで最大5ツール並列実行
- フォローアップLLMコールに `tools` を渡す（最終イテレーション以外）
- サンドボックス: `/tmp/sandbox/{session_key}/`、`_sandbox_dir` 自動注入

## Tool Integration (integrations.rs)
- `ToolPermission::AutoApprove` — web_search, calculator, datetime等
- `ToolPermission::RequireConfirmation` — gmail send, github create, phone call等
- 新ツール追加時: `Tool` trait実装 + `permission()` + `confirmation_message()`

## Provider (provider/)
- `LoadBalancedProvider` でフェイルオーバー
- `get_smartest_model()`: Opus > Gemini Pro > GPT-4o > Sonnet（要約・高品質タスク用）
- クレジット計算: `calculate_credits()` は切り上げ除算、最低1クレジット

## Memory (memory/)
- DynamoDB: `MEMORY#{user_id}` + sk: `LONG_TERM` or `DAILY#{date}`
- チャット開始時にsystem promptへ注入
- 会話終了後にデイリーログへ自動追記（fire-and-forget）

## Stripe (stripe.rs)
- Feature flag: `stripe` — Cargo.toml で有効化
- 無効時はHTTP 503返却
- Webhook署名: HMAC-SHA256
- 環境変数: STRIPE_SECRET_KEY, STRIPE_WEBHOOK_SECRET, STRIPE_PRICE_*
