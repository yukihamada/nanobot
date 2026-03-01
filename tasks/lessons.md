# Lessons Learned

## 2026-02-24: Deploy with musl target
- Build target is `aarch64-unknown-linux-musl` (NOT gnu). Always use musl for Lambda.
- LTO fat linking takes 5-13 minutes. Subsequent builds are faster due to crate caching.
- Deploy script health check at `/health` may fail on cold start. Manual verification with `curl https://chatweb.ai/health` is reliable.
- API Gateway uses $LATEST directly (not alias), but deploy script still publishes versions and updates alias for tracking.
- GA4 measurement ID is `G-3YF25NMXG8` (chatweb.ai) and `G-QS0M5KL7YL` (teai.io). Both are live.
- GA4 snippet must be added to ALL HTML pages, not just index.html. 14 sub-pages were missing it.
- teai.io SPA: `handle_root()` must `.replacen("G-3YF25NMXG8", "G-QS0M5KL7YL", 2)` to swap GA4 tags.
- PostHog was dead code (placeholder `YOUR_PROJECT_KEY`). Removed from index.html, pricing.html, teai-pricing.html, teai-index.html.
- A/B test events: bridge to GA4 via `gtag('event', ...)` inside `AB.track()` so analytics aren't siloed in DynamoDB only.

## 2026-02-24: Frontend timeout for agentic mode
- 30s frontend timeout was too short for agentic mode (multi-iteration tool loop). Increased to 90s.
- The timeout is cancelled once SSE stream connects, so this only affects connection time, not total response time.


## Nemotron Streaming Bug (2026-02-28)
- **問題**: `enable_thinking: false` を vLLM に送ると `</think>` タグが出ない
- **バグ**: `think_done = !is_runpod` → RunPodは `think_done=false` でバッファしたまま全ストリームを捨てていた
- **修正**: `think_done = true` (常にコンテンツ直送) — `openai_compat.rs:chat_stream()`
- **発見**: `curl`で `/v1/chat/completions stream=true` したら `delta: {}` (空) だったが非ストリームは正常

## Nemotron Pod スループット (2026-02-28)
- **問題**: `max_model_len=131072, max_num_seqs=4` → KVキャッシュ巨大 → 同時4リクエストで詰まる
- **解決**: `max_model_len=8192, max_num_seqs=32` → 32同時推論可能
- **教訓**: コンテキスト長と同時スループットはトレードオフ。多ユーザー環境では短めを選ぶ

## pricing.rs case-sensitivity bug (2026-03-01)
- **問題**: `lookup_model("nvidia/NVIDIA-Nemotron-Nano-9B-v2-Japanese")` が失敗
- **原因**: PRICING_TABLE エントリが混合ケースだが、比較がケースセンシティブだった
- **修正**: `p.model.to_lowercase() == lower` に変更（PR: v138）
- **影響**: Nemotron が誤課金（5/1k → 正しくは 1/1k）。123 credits → 28 credits に修正

## Nemotron tool naming (2026-03-01)
- **問題**: Nemotronが `web_fetch` と `qr_code` を呼ばず「利用できません」と返す
- **原因**: これらの名前はNemotronの学習データで少ない → ツール名を知らない
- **修正**: `web_fetch` → `read_webpage`, `qr_code` → `create_qr` に全ファイルでリネーム（v139）
- **修正ファイル**: integrations.rs, auth.rs, tool_permissions.rs, saas_tools.rs, tool/web.rs, http.rs, web/index.html, web/skills.html, web/teai-index.html, tests/test_capabilities.sh
- **注意**: ツール名変更時は8ファイル以上に影響する。grep で漏れを確認すること

## 並行ビルドの危険性 (2026-03-01)
- **問題**: 複数の `deploy-fast.sh` を同時に起動するとビルドが競合してKILL 9される
- **教訓**: デプロイ前に `ps aux | grep deploy-fast` でアクティブなプロセスを確認
- **対策**: 前のデプロイが完了してから次のデプロイを開始する

## date_time vs datetime ツール名不一致 (2026-03-01)
- **バグ**: `auth.rs:allowed_tools()` と `tool_permissions.rs` で `"date_time"` を使用
- **正しい名前**: `integrations.rs` の実際のツール名は `"datetime"`（アンダースコアなし）
- **修正**: 両ファイルで `"date_time"` → `"datetime"` に変更（v140）
- **影響範囲**: Free プランの `allowed_tools()` 出力と auto-approve リスト
