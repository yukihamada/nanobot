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
         → AWS Lambda (nanobot-prod, ARM64, ap-northeast-1)  ← 関数名はnanobot-prod
         → DynamoDB (セッション、ユーザー、設定、メモリ)
         → LLM (OpenRouter primary + Anthropic/OpenAI fallback) ← LoadBalancedProvider
```

## ディレクトリ構成
```
crates/nanobot-core/src/
  service/http.rs         — メインHTTPハンドラー（チャット、認証、課金、TTS、メモリ、スキル等）
  service/commands.rs     — スラッシュコマンド（/help, /status, /share, /link, /improve）
  service/integrations.rs — ツール統合（web_search, calculator, code_execute等 50種類以上）
  service/auth.rs         — 認証・クレジット計算
  channel/line.rs         — LINE Webhook
  channel/telegram.rs     — Telegram Webhook
  channel/facebook.rs     — Facebook Messenger Webhook
  provider/mod.rs         — LoadBalancedProvider（フェイルオーバー、サーキットブレーカー）
  provider/openai_compat.rs — OpenAI互換プロバイダー（OpenRouter/DeepSeek/Groq等）
  memory/                 — 長期記憶バックエンド（DynamoDB / ファイル）
web/
  index.html              — メインWebフロントエンド（SPA、全HTMLインライン）
  skill.html              — スキルマーケットプレイス（公開/インストール/マイスキル）
  pricing.html            — 料金ページ（クーポンUI付き）
infra/
  template.yaml           — SAMテンプレート
  deploy-fast.sh          — 高速デプロイスクリプト（本番: LAMBDA_FUNCTION_NAME=nanobot-prod）
```

## LLMモデル構成（2026-02現在）

### Webチャンネルデフォルト（Normalティア）
| 優先順 | モデル | ルート |
|--------|--------|--------|
| 1st | `minimax/minimax-m2.5` | OpenRouter |
| 2nd | `moonshotai/kimi-k2.5` | OpenRouter |
| 3rd (fail) | `openai/o4-mini` | OpenRouter |
| 4th (fail) | `claude-sonnet-4-6` | Anthropic native |

### ティア定義（`get_tier_model`）
- **economy**: gemini-2.5-flash → deepseek-chat → qwen3-32b
- **normal**: minimax-m2.5 → kimi-k2.5 → o4-mini → claude-sonnet-4-6 ← **デフォルト**
- **powerful**: claude-sonnet-4-6 → gpt-4o → gemini-2.5-pro

### 重要バグ修正（2026-02）
**`provider/openai_compat.rs` の `normalize_model`**:
- OpenRouterは `minimax/minimax-m2.5` のようにプロバイダープレフィックスが必要
- `api_base.contains("openrouter.ai")` の場合はモデル名をそのまま渡す
- 他のネイティブAPIはプレフィックスを削除（`minimax/` → `minimax-m2.5`）

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

## スキルマーケットプレイス（2026-02追加）
認証済みユーザーが誰でも外部スキルを公開・配布できる仕組み。

### スキルタイプ
- **prompt型**: Markdownをsystem promptに注入（キャラクター・専門知識等）
- **tool型**: LLMが呼び出せるWebhook（外部API/サービスをツールとして公開）

### DynamoDBパターン
- `SKILL#{id}/INFO` — スキル本体
- `SKILL_INDEX/SK#{id}` — 全スキル一覧用インデックス
- `SKILL_AUTHOR#{user_id}/SK#{id}` — 自分のスキル一覧
- `USER_SKILL#{user_id}/SK#{id}` — インストール済みスキル（webhook_url, parameters_schema含む）

### APIエンドポイント（スキル）
- `GET /api/v1/skills` — 公開スキル一覧
- `POST /api/v1/skills/publish` — スキル公開（認証済み全ユーザー）
- `GET /api/v1/skills/mine` — 自分のスキル一覧
- `GET/PUT/DELETE /api/v1/skills/{id}` — スキル詳細/更新/削除（所有者のみ）
- `POST/DELETE /api/v1/skills/{id}/install` — インストール/アンインストール

### Webhook呼び出し（tool型スキル）
- LLMがツール呼び出し → `call_webhook(url, tool_name, args)` → 外部HTTPS POST
- レスポンスをツール結果としてLLMに返す（30秒タイムアウト）
- `load_user_webhook_tools()` がチャット開始時にユーザーのwebhookツールをロード

## A/Bテスト（2026-02追加）
### フロントエンド（index.html）
```javascript
const AB = { assign(testId, variants), variant(testId), track(event, props) }
```
- `_ab_uid` をlocalStorageに保存（ランダム14文字）
- `hash(uid + testId) % variants.length` で確定的バリアント割り当て
- `AB.track()` → `POST /api/v1/ab/event`（fire-and-forget）

### 実施中のテスト
| テストID | バリアント | 内容 |
|---------|-----------|------|
| `guest_turns` | '0' / '2' | ゲストの無料ターン数 |
| `hero_cta` | 'control' / 'gift' | CTAボタン文言（🎁付き） |
| `auth_value_prop` | 'control' / 'value' | 認証モーダルのギフトバナー |

### バックエンド
- `POST /api/v1/ab/event` — CROイベント記録（`{ event, uid, ts, ...tests }`）
- DynamoDB: `AB_CRO#{event}/DAY#{date}` 集計 + `AB_CRO#{event}/UID#...` 個別記録
- 90日TTL

## Agentic Mode（マルチイテレーション・ツール実行）

### SSEイベント（handle_chat_stream）
| イベント | フィールド |
|---------|-----------|
| `tool_start` | tool, iteration, max_iter, args_preview |
| `tool_result` | tool, result(500字), iteration, duration_ms, is_error, is_no_results |
| `thinking` | iteration, max_iter, tool_count |
| `content_chunk` | text |
| `content` | content, model_used, tools_used, credits_remaining等 |
| `done` | - |

### フロントエンド エージェント表示
- **TOOL_ICONS**: ツール名→絵文字マップ（30種類）
- **TOOL_LABELS**: ツール名→日本語ラベル
- **実行中**: ツールアイコン + ラベル + パルスアニメ + 引数プレビュー
- **完了**: ✅/❌/⚠️ + ラベル + 実行時間バッジ（`0.8s`）
- **結果**: `<details>` で折りたたみ可能なプレビュー
- **ヘッダー**: 「⚙️ ステップ 1/3」→「🤔 ステップ 1/3 — 分析中」

### ループ構造
- **ループ**: `while current.has_tool_calls() && iteration < max_iterations`
- **プラン別制限**: Free=1回, Starter=3回, Pro/Enterprise=5回
- 各イテレーションで最大5ツールを並列実行（`futures::future::join_all`）
- **サンドボックス**: `/tmp/sandbox/{session_key}/`
- `_sandbox_dir`, `_session_key`, `_user_id`, `_refresh_token` を自動注入

## 主要な制約・注意事項

### デプロイ
- **Lambda関数名**: `nanobot-prod`（`nanobot`ではない）
- **エイリアス**: `live` → 最新バージョン
- **ビルド番号**: `b{N}` = gitコミット数（`git rev-list --count HEAD`）。Lambdaバージョン番号(v7等)とは別物
- 高速デプロイ:
  ```
  LAMBDA_FUNCTION_NAME=nanobot-prod ./infra/deploy-fast.sh
  LAMBDA_FUNCTION_NAME=nanobot-prod ./infra/deploy-fast.sh --skip-build  # コード変更なしの場合
  ```

### ビルド
- `include_str!()` でHTMLをバイナリに埋め込み → **HTML変更後は必ずリビルド**
- クロスコンパイル（HomebrewのrustcではなくrustupのRUSTCを使うこと）
- `cargo check -p nanobot-core` でコンパイルエラー確認（clippy warningは既存のもので無視可）

### コード規約
- `ChatResponse`を変更する場合、全ての`Json(ChatResponse {...})`箇所を更新すること
- CORS: chatweb.aiとapi.chatweb.aiは同一Lambdaから配信 → 相対URLを使用
- `deduct_credits()` は差し引いたクレジット数を返す。残高は別途 `get_or_create_user()` で取得
- **OpenRouterにモデルを追加する場合**: `normalize_model` はOpenRouterにはプレフィックス付きで渡す（`prov_is_openrouter`チェック済み）

### DynamoDB PKパターン（全）
| PK | SK | 内容 |
|----|-----|------|
| `USER#{id}` | `INFO` | ユーザー情報 |
| `AUTH#{token}` | `SESSION` | 認証トークン |
| `USAGE#{id}#{date}` | - | 日次使用量 |
| `MEMORY#{id}` | `LONG_TERM` / `DAILY#{date}` | 長期記憶 |
| `SHARE#{hash}` | `INFO` | 共有会話 |
| `CONV_SHARE#{conv_id}` | `INFO` | 会話→共有ハッシュ逆引き |
| `SKILL#{id}` | `INFO` | スキル定義 |
| `SKILL_INDEX` | `SK#{id}` | スキル全件インデックス |
| `SKILL_AUTHOR#{user_id}` | `SK#{id}` | 著者別スキル |
| `USER_SKILL#{user_id}` | `SK#{id}` | インストール済みスキル |
| `COUPON#{code}` | `INFO` | クーポン定義 |
| `REDEEM#{user}#{code}` | `INFO` | クーポン使用済み |
| `AB_CRO#{event}` | `DAY#{date}` / `UID#...` | A/Bテストイベント |
| `AB_STATS#global` | `CURRENT` / `DAY#{date}` | バリアント統計 |
| `AUDIT#{date}` | - | 監査ログ（90日TTL） |

### チャネル別動作
| チャネル | モデル | プロンプト特性 |
|---------|--------|--------------|
| Web | minimax/minimax-m2.5 (Normal tier) | 詳細・ツール使用可・音声中心 |
| LINE | デフォルト(高速) | 200字以内、絵文字、箇条書き |
| Telegram | デフォルト(高速) | 300字以内、Markdown記法 |
| Facebook | デフォルト(高速) | 300字以内、簡潔 |

### APIエンドポイント（全）
- `POST /api/v1/chat` — メインチャット
- `POST /api/v1/chat/stream` — SSEストリーミング
- `POST /api/v1/chat/race` — マルチモデルレース（tier: economy/normal/powerful）
- `POST /api/v1/chat/explore` — 全モデル並行実行
- `POST /api/v1/speech/synthesize` — TTS
- `GET /api/v1/auth/me` — 認証確認
- `GET/POST /api/v1/skills/*` — スキルマーケットプレイス
- `POST /api/v1/ab/event` — A/Bテストイベント記録
- `GET /api/v1/ab/stats` — A/Bテスト統計
- `POST /api/v1/coupon/redeem` — クーポン適用
- `GET /c/{hash}` — 共有会話ページ
- `POST /webhooks/line|telegram|facebook|stripe` — Webhook

### スラッシュコマンド
| コマンド | 動作 | 権限 |
|---------|------|------|
| `/help` | コマンド一覧 | 全員 |
| `/status` | LLMプロバイダー状態 | 全員 |
| `/share` | 会話共有URL生成 | 認証済み |
| `/link [CODE]` | チャネル連携 | 全員 |
| `/improve <説明>` | 自己改善PR | 管理者のみ |

### チャネル連携
- **LINE Bot ID**: @619jcqqh, deep link: `https://line.me/R/oaMessage/@619jcqqh/`
- **Telegram Bot**: @chatweb_ai_bot, deep link: `https://t.me/chatweb_ai_bot?start=<session_id>`

### サンドボックスツール
- Lambda AL2023にはPython/Node.jsなし → `language='shell'` が最も安全
- shell で awk/bc/sed が利用可能
- タイムアウト: ツール実行25秒、コード実行10秒

### クーポン・クレジット
- Free plan: サインアップ時100クレジット付与
- クーポン `HAMADABJJ` = 1000クレジット追加

### Local LLM Fallback
- Feature flag: `local-fallback` (Cargo.toml)
- 全リモートAPI失敗時の最終フォールバック（0クレジット）
- 環境変数: `LOCAL_MODEL_URL`, `LOCAL_TOKENIZER_URL`

### 既知の問題・注意
- Web Speech APIはChrome/Edgeのみ対応、Firefox/Safariは自動非表示
- `code_execute`はshellのみ確実動作（Python/Node.jsはLambda環境にない）
- OpenAI APIキーのクォータ切れに注意（native OpenAI providerが全滅するとフォールバックが必要）
- `google/gemini-3-flash-preview` は存在しない → `google/gemini-2.5-flash-preview` を使う
