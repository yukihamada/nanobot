# Implementation Plan: Explore Mode リアルタイムストリーミング

## 目標
複数LLMプロバイダーの並列実行で、最初に完了したものから順次SSEストリーミングで返す。

## 現状の問題
```rust
// http.rs:6468-6471
let response_stream = futures::stream::once(async move {
    let results = lb_raw.chat_race(...).await; // ❌ 全プロバイダー完了まで待つ
    // ...全結果を一括でJSON化
    Ok(Event::default().data(serde_json::json!({
        "results": events_json // ❌ 一度に全部送信
    })))
});
```

**問題点**:
- Claude: 2秒完了 → 10秒待たされる
- Gemini: 3秒完了 → 10秒待たされる
- GPT-4o: 10秒完了 → ようやく表示

## 修正方針

### Phase 1: provider/mod.rs — chat_race_stream 追加

**新関数**: `chat_race_stream(messages, tools, max_tokens, temperature) -> mpsc::Receiver<RaceResult>`

```rust
// provider/mod.rs:476行目あたりに追加
pub async fn chat_race_stream(
    &self,
    messages: &[Message],
    tools: Option<&[serde_json::Value]>,
    max_tokens: u32,
    temperature: f64,
) -> tokio::sync::mpsc::Receiver<RaceResult> {
    let parallel_models = self.available_parallel_models();
    let rank_counter = Arc::new(AtomicUsize::new(1));
    let (tx, rx) = tokio::sync::mpsc::channel::<RaceResult>(parallel_models.len() + 1);
    let msgs = messages.to_vec();
    let tools_owned: Option<Vec<serde_json::Value>> = tools.map(|t| t.to_vec());

    for (model_name, idx) in &parallel_models {
        let provider = self.providers[*idx].clone();
        let model = model_name.clone();
        let msgs = msgs.clone();
        let tools = tools_owned.clone();
        let tx = tx.clone();
        let rank_counter = rank_counter.clone();
        tokio::spawn(async move {
            let start = std::time::Instant::now();
            let tools_ref = tools.as_deref();
            match tokio::time::timeout(
                std::time::Duration::from_secs(600),
                provider.chat(&msgs, tools_ref, &model, max_tokens, temperature),
            ).await {
                Ok(Ok(resp)) => {
                    let elapsed = start.elapsed().as_millis() as u64;
                    let rank = rank_counter.fetch_add(1, Ordering::SeqCst);
                    tracing::info!("Race stream: {} finished rank={} in {}ms", model, rank, elapsed);
                    // ✅ 完了したら即座に送信
                    let _ = tx.send(RaceResult {
                        model: model.clone(),
                        response: resp.content.unwrap_or_default(),
                        response_time_ms: elapsed,
                        input_tokens: resp.usage.prompt_tokens,
                        output_tokens: resp.usage.completion_tokens,
                        rank,
                        is_fallback: false,
                    }).await;
                }
                Ok(Err(e)) => {
                    tracing::warn!("Race stream: {} failed: {}", model, e);
                }
                Err(_) => {
                    tracing::warn!("Race stream: {} timed out (10s)", model);
                }
            }
        });
    }

    // Local fallback (optional)
    #[cfg(feature = "local-fallback")]
    {
        if let Some(local_provider) = local::LocalProvider::from_env() {
            let msgs = msgs.clone();
            let tx = tx.clone();
            let rank_counter = rank_counter.clone();
            tokio::spawn(async move {
                let start = std::time::Instant::now();
                match local_provider.chat(&msgs, None, "local-qwen3-0.6b", max_tokens.min(512), temperature).await {
                    Ok(resp) => {
                        let elapsed = start.elapsed().as_millis() as u64;
                        let rank = rank_counter.fetch_add(1, Ordering::SeqCst);
                        let _ = tx.send(RaceResult {
                            model: "local-qwen3-0.6b".to_string(),
                            response: resp.content.unwrap_or_default(),
                            response_time_ms: elapsed,
                            input_tokens: resp.usage.prompt_tokens,
                            output_tokens: resp.usage.completion_tokens,
                            rank,
                            is_fallback: true,
                        }).await;
                    }
                    Err(e) => {
                        tracing::warn!("Race stream: local fallback failed: {}", e);
                    }
                }
            });
        }
    }

    // tx を drop して rx が終了を検知できるようにする
    drop(tx);

    rx
}
```

**変更点**:
- `Vec<RaceResult>` を返す代わりに `mpsc::Receiver<RaceResult>` を返す
- 各プロバイダー完了時に `tx.send()` で即座に送信
- 呼び出し側は `rx.recv()` でストリーミング受信

---

### Phase 2: http.rs — handle_chat_explore をストリーミング化

**変更箇所**: `crates/nanobot-core/src/service/http.rs:6291-6550`

```rust
async fn handle_chat_explore(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ExploreRequest>,
) -> impl IntoResponse {
    use axum::response::sse::{Event, Sse};
    use std::convert::Infallible;

    // ... (入力検証、session_key解決、クレジット確認は変更なし)

    // ... (メッセージ構築も変更なし)

    // ✅ ストリーミング用のチャネルを作成
    let mut rx = lb_raw.chat_race_stream(&messages, None, max_tokens, temperature).await;

    let state_clone = state.clone();
    let session_key_clone = session_key.clone();
    let original_msg = req.message.clone();

    // ✅ async_stream でリアルタイムSSE生成
    let response_stream = async_stream::stream! {
        let start = std::time::Instant::now();
        let mut results_for_session = Vec::new();
        let mut total_credits: i64 = 0;
        let mut last_remaining: Option<i64> = None;
        let mut rank = 1;

        // ✅ 各プロバイダー完了時に即座にイベント送信
        while let Some(result) = rx.recv().await {
            results_for_session.push(result.clone());

            // クレジット差し引き
            #[cfg(feature = "dynamodb-backend")]
            {
                if let (Some(dynamo), Some(table)) = (&state_clone.dynamo_client, &state_clone.config_table) {
                    let (credits, remaining) = deduct_credits(
                        dynamo, table, &session_key_clone, &result.model,
                        result.input_tokens, result.output_tokens,
                    ).await;
                    total_credits += credits;
                    if remaining.is_some() { last_remaining = remaining; }
                }
            }

            // Kimi k2.5 優先判定
            let is_kimi = result.model.to_lowercase().contains("kimi")
                || result.model.to_lowercase().contains("moonshot");
            let is_preferred = is_kimi && result.response_time_ms <= 10_000;

            // ✅ 個別結果を即座にSSE送信
            yield Ok::<_, Infallible>(Event::default()
                .event("explore_result")
                .data(serde_json::json!({
                    "model": result.model,
                    "response": result.response,
                    "time_ms": result.response_time_ms,
                    "rank": result.rank,
                    "index": rank - 1,
                    "is_fallback": result.is_fallback,
                    "is_preferred": is_preferred,
                    "credits_used": crate::service::auth::calculate_credits(
                        &result.model, result.input_tokens, result.output_tokens
                    ),
                    "credits_remaining": last_remaining,
                }).to_string())
            );

            rank += 1;
        }

        let total_time = start.elapsed().as_millis() as u64;

        // セッション保存
        {
            let mut sessions = state_clone.sessions.lock().await;
            let session = sessions.get_or_create(&session_key_clone);
            session.add_message_from_channel("user", &original_msg, "web");
            if let Some(best) = results_for_session.first() {
                session.add_message_from_channel("assistant",
                    &format!("[Explore: {} models] {}", results_for_session.len(), best.response), "web");
            }
            sessions.save_by_key(&session_key_clone);
        }

        #[cfg(feature = "dynamodb-backend")]
        {
            if let (Some(dynamo), Some(table)) = (&state_clone.dynamo_client, &state_clone.config_table) {
                increment_sync_version(dynamo, table, &session_key_clone, "web").await;
            }
        }

        // ✅ 最後に完了イベント送信
        yield Ok::<_, Infallible>(Event::default()
            .event("explore_done")
            .data(serde_json::json!({
                "type": "done",
                "total_models": results_for_session.len(),
                "total_time_ms": total_time,
                "total_credits_used": total_credits,
                "credits_remaining": last_remaining,
            }).to_string())
        );
    };

    Sse::new(response_stream).into_response()
}
```

**変更点**:
- `futures::stream::once` → `async_stream::stream!`
- `chat_race` → `chat_race_stream`
- 各結果を `explore_result` イベントとして即座に送信
- 最後に `explore_done` イベントで完了通知

---

### Phase 3: Web UI 対応（既存コードを確認）

**ファイル**: `web/index.html` の `processExploreEvent()` 関数

**必要な変更**:
```javascript
// 現状: 単一の explore_results イベントを処理
if (event.type === 'explore_results') {
    // results 配列を一括処理
}

// 修正: 個別イベントをストリーミング処理
eventSource.addEventListener('explore_result', (e) => {
    const result = JSON.parse(e.data);
    // ✅ 1件ずつカードを即座に追加
    addExploreCard(result);
});

eventSource.addEventListener('explore_done', (e) => {
    const summary = JSON.parse(e.data);
    // ✅ 完了通知を表示
    showExploreSummary(summary);
});
```

---

## テスト計画

### 1. ユニットテスト
```bash
cd crates/nanobot-core
cargo test chat_race_stream
```

### 2. 統合テスト（ローカル）
```bash
# Lambda ローカル起動
cargo run --release -- gateway --http --http-port 3000

# curlでテスト
curl -X POST http://localhost:3000/api/v1/chat/explore \
  -H "Content-Type: application/json" \
  -d '{"message":"Rustとは何ですか？","session_id":"test"}'

# SSEイベントが順次表示されることを確認
```

### 3. 本番テスト（Lambda）
```bash
# デプロイ
./infra/deploy-fast.sh

# Web UIでExploreモードを実行
# → カードが1枚ずつ順次表示されることを確認
```

---

## ロールバックプラン

変更が問題を起こした場合:
1. `chat_race_stream` を `#[allow(dead_code)]` で無効化
2. `handle_chat_explore` を元の `futures::stream::once` に戻す
3. `git revert` で変更を取り消し

---

## 期待効果

### Before
- 全プロバイダー完了まで待つ: 10秒+
- ユーザーは白画面を10秒見続ける

### After
- 最速プロバイダー: 2秒で表示 ✨
- 2番目: 3秒で追加表示 ✨
- 3番目以降: 順次追加 ✨
- 体感速度: **3-5倍向上**

---

## 次のステップ

1. ✅ 実装計画レビュー（このファイル）
2. ⏳ provider/mod.rs 実装（chat_race_stream 追加）
3. ⏳ http.rs 実装（handle_chat_explore 改修）
4. ⏳ ローカルテスト
5. ⏳ デプロイ & 本番確認
