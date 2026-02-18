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
  service/commands.rs   — スラッシュコマンドフレームワーク（/help, /status, /share, /link, /improve）
  service/integrations.rs — ツール統合（web_search, calculator, code_execute, file_read/write/list等）
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
- クロスコンパイル（HomebrewのrustcではなくrustupのRUSTCを使うこと）:
  ```
  RUSTUP_TOOLCHAIN=stable RUSTC=/Users/yuki/.rustup/toolchains/stable-aarch64-apple-darwin/bin/rustc \
    cargo zigbuild --manifest-path crates/nanobot-lambda/Cargo.toml --release --target aarch64-unknown-linux-gnu
  ```
- 高速デプロイ（SAM不要）:
  ```
  cd target/lambda/bootstrap && zip -j /tmp/nanobot-lambda.zip bootstrap
  aws lambda update-function-code --function-name nanobot --zip-file "fileb:///tmp/nanobot-lambda.zip" --region ap-northeast-1
  aws lambda publish-version --function-name nanobot --region ap-northeast-1
  aws lambda update-alias --function-name nanobot --name live --function-version <VER> --region ap-northeast-1
  ```

### コード規約
- `ChatResponse`を変更する場合、全ての`Json(ChatResponse {...})`箇所を更新すること
- CORS: chatweb.aiとapi.chatweb.aiは同一Lambdaから配信 → 相対URLを使用
- DynamoDB PKパターン: `USER#{id}`, `AUTH#{token}`, `USAGE#{id}#{date}`, `MEMORY#{id}`, `SHARE#{hash}`, `CONV_SHARE#{conv_id}`
- `deduct_credits()` は差し引いたクレジット数を返す。残高は別途 `get_or_create_user()` で取得

### チャネル別動作
| チャネル | モデル | プロンプト特性 |
|---------|--------|--------------|
| Web | claude-sonnet-4-5 (自動) | 詳細・ツール使用可・音声中心 |
| LINE | デフォルト(高速) | 200字以内、絵文字、箇条書き |
| Telegram | デフォルト(高速) | 300字以内、Markdown記法 |
| Facebook | デフォルト(高速) | 300字以内、簡潔 |

### スラッシュコマンド（commands.rs）
全チャネル共通（Web/LINE/Telegram）。`commands::parse_command()` + `execute_command()` で一元管理。

| コマンド | 動作 | 権限 |
|---------|------|------|
| `/help` | コマンド一覧 | 全員 |
| `/status` | LLMプロバイダー状態（レイテンシms付き） | 全員 |
| `/share` | 会話の共有URL生成（`/c/{hash}`） | 認証済み |
| `/link [CODE]` | チャネル連携 | 全員 |
| `/improve <説明>` | 自己改善PR（準備中） | 管理者のみ |

### 会話共有リンク
- **ハッシュ**: UUID v4 → base62エンコード10文字（推測不可能）
- **DynamoDB**: `SHARE#{hash}` → conv_id, user_id, revoked
- **逆引き**: `CONV_SHARE#{conv_id}` → hash（重複防止）
- **取消**: `revoked = true` で無効化（DELETE API）
- **共有ビュー**: `/c/{hash}` → SPA読み取り専用モード（入力非表示、CTA表示）

### API エンドポイント
- `POST /api/v1/chat` — メインチャット（credits_used/credits_remaining返却、スラッシュコマンド対応）
- `POST /api/v1/chat/stream` — SSEストリーミング応答
- `POST /api/v1/speech/synthesize` — TTS（OpenAI tts-1, nova）
- `GET /api/v1/auth/me` — 認証確認 + クレジット情報
- `GET /c/{hash}` — 共有会話ページ（SPA、読み取り専用）
- `GET /api/v1/shared/{hash}` — 共有会話メッセージJSON（公開、認証不要）
- `POST /api/v1/conversations/{id}/share` — 共有リンク生成（Bearer認証）
- `DELETE /api/v1/conversations/{id}/share` — 共有リンク無効化（Bearer認証）
- `POST /webhooks/line` — LINE Webhook
- `POST /webhooks/telegram` — Telegram Webhook
- `GET/POST /webhooks/facebook` — Facebook Messenger Webhook
- `POST /webhooks/stripe` — Stripe Webhook
- `POST /api/v1/chat/explore` — マルチモデル並行実行（SSE, 階層的再問い合わせ Lv0-2）
- `POST /api/v1/coupon/redeem` — クーポンコード適用

### Agentic Mode（マルチイテレーション・ツール実行）
- `handle_chat` / `handle_chat_stream` がマルチイテレーションツールループを実装
- **ループ構造**: `while current.has_tool_calls() && iteration < max_iterations`
- **プラン別制限**: Free=1回, Starter=3回, Pro/Enterprise=5回 (`Plan::max_tool_iterations()`)
- 各イテレーションで最大5ツールを並列実行、クレジットは各LLMコール毎に累積差引
- フォローアップLLMコールに `tools` を渡す（最終イテレーション以外）→ LLMが更にツールを呼べる
- **サンドボックス**: `/tmp/sandbox/{session_key}/` にセッション別ディレクトリ作成
- `_sandbox_dir` パラメータをツール呼び出し時に自動注入

#### サンドボックスツール（integrations.rs）
| ツール | 説明 | 制限 |
|--------|------|------|
| `code_execute` | shell/python/nodejs実行 | 10秒タイムアウト、安全ガードパターン |
| `file_read` | サンドボックス内ファイル読取 | パストラバーサル禁止、32KB上限 |
| `file_write` | サンドボックス内ファイル書込 | 100KB上限、親ディレクトリ自動作成 |
| `file_list` | ディレクトリ一覧 | サンドボックス内のみ |

- **重要**: Lambda AL2023にはPython/Node.jsなし → `code_execute`は`language='shell'`が最も安全
- shell で awk/bc/sed が利用可能（計算・テキスト処理に十分）
- Python/Node.js は `which` で検出、なければエラーメッセージでshellへ誘導

#### SSEストリーミング進捗（handle_chat_stream）
- イベント型: `start`, `tool_start`, `tool_result`, `thinking`, `content`, `error`, `done`
- JSON配列として単一SSEイベントで送信（API Gateway v2互換）
- Web UIの `processEvent()` が配列を展開して各イベントを処理
- `.agent-progress` CSSクラスでツール実行ステップを表示

#### プラン別ツール制限（auth.rs）
| プラン | ツール | イテレーション | サンドボックス |
|--------|--------|---------------|--------------|
| Free | 基本ツールのみ（web_search, calculator等） | 1 | なし |
| Starter | 全ツール | 3 | あり |
| Pro | 全ツール | 5 | あり |

### Explore Mode（マルチモデル探索）
- `POST /api/v1/chat/explore` — 全プロバイダーを並行実行し、全結果をSSEで返す
- 各モデルの結果ごとにクレジットが差し引かれる（`deduct_credits` per result）
- 階層的プロンプト: level=0（直接）, level=1（ステップバイステップ）, level=2（専門家分析）
- `calculate_credits()` は切り上げ除算 — 少トークンでも最低1クレジット消費
- SSEは `futures::stream::once()` パターン（API Gateway v2互換、async_streamは不可）
- Free plan も使用可能（クレジット消化促進 → アップグレード誘導）

### クーポン・クレジット
- Free plan: サインアップ時100クレジット付与
- クーポン `HAMADABJJ` = 1000クレジット追加
- DynamoDB: `COUPON#{code}` (設定), `REDEEM#{user}#{code}` (重複防止)

### チャネル連携
- **LINE Bot ID**: @619jcqqh, deep link: `https://line.me/R/oaMessage/@619jcqqh/`
- **Telegram Bot**: @chatweb_ai_bot, deep link: `https://t.me/chatweb_ai_bot?start=<session_id>`
- Web UI: LINEボタン → QRコードモーダル → 自動連携
- `/link` コマンド: 6文字コード発行 → 別チャネルで `/link <code>` で連携

### フロントエンド（web/index.html）
- 全てインラインSPA（CSS + HTML + JS が1ファイル）
- **Voice-first**: 大きなマイクボタンが主要CTA、Push-to-talk体験
- STT: Web Speech API (ja-JP)、マイクボタン、自動送信
- TTS: /api/v1/speech/synthesize → MP3再生 + キャッシュ
- Auto-TTS: 音声入力→応答を自動で音声再生
- SSE: /api/v1/chat/stream でリアルタイム応答（エージェント進捗: tool_start/tool_result/thinking イベント対応）
- クレジット表示: サイドバー、応答ごとにリアルタイム更新
- appAddMsg() でbotメッセージにTTSボタン自動付与
- 共有ビュー: `/c/{hash}` URL検出 → 読み取り専用モード（サイドバー・入力非表示、CTAボタン）
- 共有ボタン: サイドバーの会話リスト各項目にホバー表示リンクアイコン → クリップボードコピー + トースト
- Explore モード: トグル ON → `/api/v1/chat/explore` SSE → 全モデル結果をカードで表示
- クーポン入力: 認証モーダルにクーポンフィールド追加

### CI/CD
- `main`へのpushで自動デプロイ（test → build → canary → production）
- PRにはCIテスト通過 + 1 approving review が必要
- `enforce_admins: false` → オーナーはバイパス可能

### Local LLM Fallback
- Feature flag: `local-fallback` (Cargo.toml)
- candle + Qwen3-0.6B GGUF (Q4_K_M, ~350MB)
- 全リモートAPI失敗時の最終フォールバック
- 0クレジット（`credit_rate` で `local-` prefix は無料）
- 環境変数: `LOCAL_MODEL_URL`, `LOCAL_TOKENIZER_URL`

### 既知の問題
- `integrations.rs` の `base64` クレートが未追加（コンパイルエラー、http.rsとは無関係）
- Web Speech APIはChrome/Edgeのみ対応、Firefox/Safariは自動非表示
- `strip_prefix_ci`: バイトインデックスでマルチバイト文字境界パニックを起こす → `.get()` で安全化済み
- Lambda AL2023にPython/Node.jsなし → `code_execute`は`shell`のみ確実に動作
- テスト `test_tool_registry_builtins` はツール数15（サンドボックス4追加後）、GitHub込み18
