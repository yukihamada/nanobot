# Email Verification for teai.io Registration

## 概要
teai.io の `/register` フローにメール認証を追加する。現在は `handle_auth_register` がアカウントを即座に作成しているが、メール認証コードの確認後にアカウント作成するよう変更する。

## 調査結果
- **関連ファイル**:
  - `crates/nanobot-core/src/service/http.rs` — `handle_auth_register` (L16949), `handle_auth_verify` (L17498), `send_verification_email` (L17233)
  - `web/teai-register.html` — 登録/ログインフォーム (L1-144)
- **既存パターン**: `handle_auth_email` (L17268) が既にメール認証フローを実装済み。`VERIFY#{email}/CODE` にコード・TTL・attempts を保存し、`handle_auth_verify` で検証後にアカウント作成する。このパターンを `register` にも適用する。
- **DynamoDBパターン**: `VERIFY#{email}/CODE` に 6桁コード + TTL(10分) + attempts(max 3) を保存
- **Resend API**: `send_verification_email()` が動作済み（teai.io ブランド対応含む）
- **VerifyRequest**: `{ email, code, session_id?, referral_code? }` — password フィールドがない。register 用に拡張が必要
- **RegisterRequest**: `{ email, password, name?, referral_code? }`
- **注意**: `handle_auth_verify` は passwordless auth 用。register は password 付きなので、VERIFY レコードに password_hash+salt を保存する必要がある

## 実装ステップ

### Backend (http.rs)
- [ ] Step 1: `VerifyRequest` に `password` フィールド追加 (`Option<String>`)（推定: 小）
  - 既存の passwordless verify は password=None で動作し続ける
- [ ] Step 2: `handle_auth_register` を変更 — アカウント即作成 → 認証コード送信に変更（推定: 中）
  - email/password バリデーションは既存のまま
  - `EMAIL#{email}/CREDENTIALS` の重複チェックは既存のまま
  - password_hash + salt を計算し、`VERIFY#{email}/CODE` に保存（code, ttl, attempts, password_hash, salt, name, referral_code）
  - `send_verification_email()` でコード送信
  - レスポンス: `{ pending_verification: true, message: "認証コードをメールに送信しました" }`
  - RESEND_API_KEY 未設定時: 既存の即時作成フローにフォールバック
- [ ] Step 3: `handle_auth_verify` を拡張 — password_hash が VERIFY レコードにある場合、`EMAIL#{email}/CREDENTIALS` にパスワード付きアカウントを作成（推定: 中）
  - VERIFY レコードから `password_hash`, `salt` を取り出す
  - 値がある場合: `EMAIL#{email}/CREDENTIALS` に password_hash, salt, user_id を保存（register フロー）
  - 値がない場合: 既存の passwordless フロー（`auth_method: "email_verified"`）

### Frontend (teai-register.html)
- [ ] Step 4: 認証コード入力 UI を追加（推定: 中）
  - `pending_verification: true` レスポンス時、フォームを認証コード入力に切り替え
  - 6桁の数字入力欄 + 「認証する」ボタン
  - コード送信先メールアドレスの表示
  - 「コードを再送信」リンク（register を再呼び出し）
  - コード検証: `POST /api/v1/auth/verify` に `{ email, code, password }` を送信
    - ただし password は verify 時には不要（VERIFY レコードに保存済み）。email + code のみ送信
  - 成功時: token を localStorage に保存し `/dashboard` へリダイレクト（既存と同じ）

## テスト方針
- [ ] `cargo check -p nanobot-core` でコンパイル確認
- [ ] 手動テスト: curl で `/api/v1/auth/register` → `pending_verification: true` 確認
- [ ] 手動テスト: curl で `/api/v1/auth/verify` → token 返却確認
- [ ] RESEND_API_KEY 未設定時のフォールバック（即時作成）確認
- [ ] 既存の passwordless verify が壊れていないことを確認

## リスク
- `VerifyRequest` に `password` を追加すると既存の passwordless verify 呼び出しに影響する可能性 → `Option<String>` で安全
- VERIFY レコードに password_hash を保存するため、TTL(10分) 内にコードが期限切れになると再登録が必要 → 「コードを再送信」で対応
- `handle_auth_verify` の分岐が増え、1関数の責務が大きくなる → 既存パターンに合わせる（リファクタは別タスク）

## 完了条件
- 登録時にメール認証コードが送信される
- コード入力後にアカウントが作成される
- パスワード付きアカウントとして正常にログインできる
- RESEND_API_KEY 未設定時は従来の即時作成にフォールバック
- 既存の passwordless email auth (`/api/v1/auth/email` + `/api/v1/auth/verify`) が壊れない

---

# Todo — teai.io LLM API Gateway ローンチ（最終更新: 2026-03-10）

## 現状（既に使えるもの）
- [x] ランディングページ (web/teai-index.html)
- [x] Fly.io `nanobot-ai` 東京リージョン稼働中 ($0-2/月)
- [x] `/v1/chat/completions` OpenAI互換API実装済み
- [x] Stripe 本番キー設定済み
- [x] LLMキー: Anthropic, OpenRouter, Groq, RunPod (Nemotron)
- [x] nanobot-fly crate (Rust + axum + libSQL)
- [x] SSEストリーミング実装済み
- [x] ユーザー認証・クレジット課金実装済み

## Phase 1: API Gateway 基盤 (今日) ✅ ほぼ完了

### 1-1. マルチプロバイダー ルーティング
- [x] `te_` プレフィックスのAPIキー発行 (Lambda v177)
- [x] model パラメータでプロバイダー自動振り分け (既存LoadBalancedProvider)
- [x] `/v1/models` 全利用可能モデル一覧 (api.teai.io/v1/models)
- [x] `/v1/models/pricing` モデル別料金API (api.teai.io/v1/models/pricing)
- [x] 自動フォールバック（プロバイダー障害時）— CircuitBreaker実装済み

### 1-2. デプロイ
- [x] Lambda (nanobot-prod) で api.teai.io 稼働中
- [x] Fly.io (nanobot-ai) で teai.io 稼働中
- [x] ランディングページ配信確認
- [ ] Lambda再デプロイ（/dashboard + Stripeリンク + webhook修正含む）

### 1-3. Stripe 料金プラン
- [x] teai.io Pro (prod_U7Z8D3I1JAUc9c) $29/月
- [x] teai.io Business (prod_U7Z8LTKFG0oszB) $99/月
- [x] Payment Links: Pro=buy.stripe.com/5kQcN56s37w60Fq1KMefC3q, Business=buy.stripe.com/28E5kD2bN7w6gEogFGefC3r
- [x] Webhook (we_1T9K0uDqLakc8NxkSsH5ZzVt) → api.teai.io/webhooks/stripe
- [x] STRIPE_WEBHOOK_SECRET_TEAI Lambda env var追加済み
- [ ] 3% マージン適用（pricing.rs確認）

## Phase 2: ダッシュボード ✅ 完了
- [x] API使用量ダッシュボード (web/teai-dashboard.html)
- [x] APIキー管理UI (作成/削除/一覧)
- [x] モデル一覧動的フェッチ
- [ ] APIドキュメントページ (/docs)

---

# (以前のタスク: 課金転換率改善プロジェクト 2026-03-01)

## 1. オンボーディング改善
- [x] 初回訪問時のウェルカムメッセージ改善（サジェストピル追加）
- [x] 機能デモ（音声入力・画像生成等）の自動提案（ピルクリックで送信）
- [x] プログレスバー（残りクレジット表示）（ヘッダーに4pxバー追加）

## 2. 課金導線の強化
- [x] クレジット残少時のソフトナッジ（残り20%で赤、40%でオレンジ表示）
- [x] クレジット切れ時のアップグレードモーダル改善（delay 1200ms→即時、コピー改善）
- [x] 価値体験後の自然な誘導（画像/音楽/動画生成完了後のソフトアップセル）

## 3. リテンション施策
- [x] LINE/Telegram でのデイリーサマリー通知（POST /api/v1/cron/daily-summary, admin認証, LINK#スキャン→使用量集計→push送信）
- [x] 「昨日の会話の続き」導線（空状態にリジュームボタン表示、1-48h以内の会話対象）

## 4. SEO / ランディングページ
- [x] meta tags, OGP, structured data（完備: OGP, Twitter Cards, JSON-LD, hreflang 7言語）
- [x] ヒーローセクション刷新（統計数値更新: 10+モデル, 14+チャネル, 30+ツール, 500+ユーザー）
- [x] 機能紹介セクション追加（「1行でできます」6カード + クリックで入力補完、v141）

## 5. ボイス体験の磨き込み
- [x] TTS応答速度の最適化（Replicate+QWENを並列レース化）
- [x] 音声入力→応答のレイテンシ改善（web_search並列化、agentic deadline追加）

## 6. モデル選択の動的化（追加完了）
- [x] PRICING_TABLEをSSoTとしてAPI `/api/v1/models` から全モデル返却
- [x] フロントエンドのドロップダウンが動的フェッチ（フォールバック付き）
- [x] OpenAI互換 `/v1/models` もPRICING_TABLEから動的生成

## 7. Nemotron ツール名修正（2026-03-01 追加、v139）
- [x] pricing.rs ケースセンシティブバグ修正（Nemotron 4.4x過課金 → 修正済み、v138）
- [x] web_fetch → read_webpage, qr_code → create_qr リネーム（Nemotronが認識できない名前を修正）
- [x] AGENT_COMMON プロンプト: ツール一覧と優先使用の指示を追加
- [x] tests/test_capabilities.sh 20項目テストスクリプト作成
- [x] date_time → datetime ツール名不一致バグ修正（v140予定）
- [x] tool descriptions のレガシー名参照修正（integrations.rs, v140予定）
