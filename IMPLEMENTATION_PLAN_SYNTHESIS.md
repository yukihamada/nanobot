# Implementation Plan: メタ分析モード（統合分析）

## 目標
Explore Mode で複数LLMの回答を収集後、最も賢いモデル（Claude Opus 4.6 / GPT-4o）がそれらを総合分析して意見を述べる。

**両方対応**:
- ✅ 自動統合分析（`auto_synthesize: true`）
- ✅ 手動統合分析（「📊 統合分析」ボタン）

---

## Phase 1: データ構造追加

### 1.1 ExploreRequest 拡張

**ファイル**: `crates/nanobot-core/src/service/http.rs`

```rust
#[derive(Debug, Deserialize)]
pub struct ExploreRequest {
    pub message: String,
    #[serde(default = "default_session_id")]
    pub session_id: String,
    #[serde(default)]
    pub previous_chunk: Option<String>,
    pub level: Option<u8>,

    // ✅ 新フィールド: 自動統合分析
    #[serde(default)]
    pub auto_synthesize: bool,
}
```

### 1.2 ExploreSynthesizeRequest 追加

```rust
/// Request for manual synthesis of explore results.
#[derive(Debug, Deserialize)]
pub struct ExploreSynthesizeRequest {
    pub question: String,
    pub results: Vec<SynthesisInput>,
    #[serde(default = "default_session_id")]
    pub session_id: String,
}

#[derive(Debug, Deserialize)]
pub struct SynthesisInput {
    pub model: String,
    pub response: String,
}
```

---

## Phase 2: 統合分析ロジック

### 2.1 共通関数: `synthesize_results`

**ファイル**: `crates/nanobot-core/src/service/http.rs` 内に追加

```rust
/// Generate synthesis prompt from multiple model results.
fn build_synthesis_prompt(question: &str, results: &[(String, String)]) -> String {
    let mut prompt = format!(
        "以下は「{}」という質問に対する、複数のAIモデルによる回答です:\n\n",
        question
    );

    for (model, response) in results {
        prompt.push_str(&format!("### [{}]\n{}\n\n", model, response));
    }

    prompt.push_str(
        "---\n\n\
        これらの回答を総合的に分析し、以下の観点で統合された意見を述べてください:\n\n\
        1. **共通する見解**: 全てのモデルが同意している点\n\
        2. **相違点**: モデル間で意見が分かれている点とその理由\n\
        3. **信頼性評価**: 最も信頼できる情報はどれか、その根拠\n\
        4. **総合的な結論**: 全体を踏まえた最終的な回答\n\n\
        できるだけ具体的に、根拠を示しながら説明してください。"
    );

    prompt
}

/// Run synthesis using the smartest available model.
async fn run_synthesis(
    lb_provider: &Arc<crate::provider::LoadBalancedProvider>,
    question: &str,
    results: &[(String, String)],
) -> Result<(String, String, u32, u32), String> {
    // Get smartest model (Opus > GPT-4o > Gemini Pro)
    let smartest = crate::provider::get_smartest_model();

    let synthesis_prompt = build_synthesis_prompt(question, results);
    let messages = vec![
        crate::types::Message::system(
            "あなたは複数のAI回答を統合分析する専門家です。\
             客観的かつ批判的に分析し、最も正確な結論を導いてください。"
        ),
        crate::types::Message::user(&synthesis_prompt),
    ];

    match lb_provider.chat(&messages, None, &smartest, 3000, 0.7).await {
        Ok(resp) => {
            let content = resp.content.unwrap_or_default();
            Ok((
                smartest,
                content,
                resp.usage.prompt_tokens,
                resp.usage.completion_tokens,
            ))
        }
        Err(e) => Err(format!("Synthesis failed: {}", e)),
    }
}
```

---

## Phase 3: 自動統合分析（handle_chat_explore 拡張）

**ファイル**: `crates/nanobot-core/src/service/http.rs:6460-`

**変更箇所**: `explore_done` イベント送信後

```rust
// Send done event with summary
yield Ok::<_, Infallible>(Event::default()
    .event("explore_done")
    .data(serde_json::json!({
        "type": "done",
        "total_models": results_for_session.len(),
        "total_time_ms": total_time,
        "total_credits_used": total_credits,
        "credits_remaining": last_remaining,
        "level": level,
        "can_escalate": level < 2,
    }).to_string())
);

// ✅ 自動統合分析（auto_synthesize が true の場合）
if req.auto_synthesize && !results_for_session.is_empty() {
    yield Ok::<_, Infallible>(Event::default()
        .event("synthesis_start")
        .data(serde_json::json!({
            "type": "synthesis_start",
            "message": "統合分析中...",
        }).to_string())
    );

    // Extract (model, response) pairs
    let synthesis_inputs: Vec<(String, String)> = results_for_session
        .iter()
        .map(|r| (r.model.clone(), r.response.clone()))
        .collect();

    match run_synthesis(&lb_raw, &original_msg, &synthesis_inputs).await {
        Ok((model, content, input_tokens, output_tokens)) => {
            // Deduct credits for synthesis
            #[cfg(feature = "dynamodb-backend")]
            {
                if let (Some(dynamo), Some(table)) = (&state_clone.dynamo_client, &state_clone.config_table) {
                    let (credits, remaining) = deduct_credits(
                        dynamo, table, &session_key_clone, &model,
                        input_tokens, output_tokens,
                    ).await;
                    total_credits += credits;
                    if remaining.is_some() { last_remaining = remaining; }
                }
            }

            yield Ok::<_, Infallible>(Event::default()
                .event("synthesis_result")
                .data(serde_json::json!({
                    "type": "synthesis",
                    "model": model,
                    "response": content,
                    "credits_used": crate::service::auth::calculate_credits(
                        &model, input_tokens, output_tokens
                    ),
                    "credits_remaining": last_remaining,
                }).to_string())
            );
        }
        Err(e) => {
            tracing::warn!("Auto synthesis failed: {}", e);
            yield Ok::<_, Infallible>(Event::default()
                .event("synthesis_error")
                .data(serde_json::json!({
                    "type": "error",
                    "message": format!("統合分析エラー: {}", e),
                }).to_string())
            );
        }
    }
}
```

---

## Phase 4: 手動統合分析エンドポイント

**ファイル**: `crates/nanobot-core/src/service/http.rs`

### 4.1 ルート追加

```rust
// http.rs のルーター定義部分（Line 2251あたり）
.route("/api/v1/chat/explore", post(handle_chat_explore))
.route("/api/v1/chat/explore/synthesize", post(handle_explore_synthesize)) // ✅ 新規
.route("/api/v1/chat/race", post(handle_chat_race))
```

### 4.2 ハンドラー実装

```rust
/// POST /api/v1/chat/explore/synthesize — Manual synthesis of explore results.
/// Takes multiple model responses and generates a unified analysis.
async fn handle_explore_synthesize(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ExploreSynthesizeRequest>,
) -> impl IntoResponse {
    use axum::response::sse::{Event, Sse};
    use std::convert::Infallible;

    // Input validation
    if req.results.is_empty() {
        let err_stream = futures::stream::once(async {
            Ok::<_, Infallible>(Event::default()
                .event("error")
                .data(serde_json::json!({
                    "type": "error",
                    "content": "No results provided",
                    "error": "No results provided"
                }).to_string()))
        });
        return Sse::new(err_stream).into_response();
    }

    // Resolve session key
    let session_key = {
        #[cfg(feature = "dynamodb-backend")]
        {
            if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
                resolve_session_key(dynamo, table, &req.session_id).await
            } else {
                req.session_id.clone()
            }
        }
        #[cfg(not(feature = "dynamodb-backend"))]
        { req.session_id.clone() }
    };

    // Check credits
    #[cfg(feature = "dynamodb-backend")]
    {
        if let (Some(dynamo), Some(table)) = (state.dynamo_client.as_ref(), state.config_table.as_ref()) {
            let user = get_or_create_user(dynamo, table, &session_key).await;
            if user.credits_remaining <= 0 {
                let content = "クレジットを使い切りました 💪 追加購入して続けましょう！";
                let err_stream = futures::stream::once(async move {
                    Ok::<_, Infallible>(Event::default()
                        .event("error")
                        .data(serde_json::json!({
                            "type": "error",
                            "content": content,
                            "error": content,
                            "action": "upgrade"
                        }).to_string()))
                });
                return Sse::new(err_stream).into_response();
            }
        }
    }

    let lb_raw = match &state.lb_raw {
        Some(lb) => lb.clone(),
        None => {
            let err_stream = futures::stream::once(async {
                Ok::<_, Infallible>(Event::default()
                    .event("error")
                    .data(serde_json::json!({
                        "type": "error",
                        "content": "No providers available",
                        "error": "No providers available"
                    }).to_string()))
            });
            return Sse::new(err_stream).into_response();
        }
    };

    let state_clone = state.clone();
    let session_key_clone = session_key.clone();
    let question = req.question.clone();
    let synthesis_inputs: Vec<(String, String)> = req.results
        .into_iter()
        .map(|r| (r.model, r.response))
        .collect();

    let response_stream = async_stream::stream! {
        yield Ok::<_, Infallible>(Event::default()
            .event("synthesis_start")
            .data(serde_json::json!({
                "type": "synthesis_start",
                "message": "統合分析中...",
            }).to_string())
        );

        match run_synthesis(&lb_raw, &question, &synthesis_inputs).await {
            Ok((model, content, input_tokens, output_tokens)) => {
                let mut last_remaining: Option<i64> = None;

                // Deduct credits
                #[cfg(feature = "dynamodb-backend")]
                {
                    if let (Some(dynamo), Some(table)) = (&state_clone.dynamo_client, &state_clone.config_table) {
                        let (_, remaining) = deduct_credits(
                            dynamo, table, &session_key_clone, &model,
                            input_tokens, output_tokens,
                        ).await;
                        last_remaining = remaining;
                    }
                }

                yield Ok::<_, Infallible>(Event::default()
                    .event("synthesis_result")
                    .data(serde_json::json!({
                        "type": "synthesis",
                        "model": model,
                        "response": content,
                        "credits_used": crate::service::auth::calculate_credits(
                            &model, input_tokens, output_tokens
                        ),
                        "credits_remaining": last_remaining,
                    }).to_string())
                );
            }
            Err(e) => {
                tracing::warn!("Manual synthesis failed: {}", e);
                yield Ok::<_, Infallible>(Event::default()
                    .event("synthesis_error")
                    .data(serde_json::json!({
                        "type": "error",
                        "message": format!("統合分析エラー: {}", e),
                    }).to_string())
                );
            }
        }
    };

    Sse::new(response_stream).into_response()
}
```

---

## Phase 5: Web UI 対応

**ファイル**: `web/index.html`

### 5.1 自動統合分析のトグル追加

```javascript
// Explore mode settings
<label>
  <input type="checkbox" id="autoSynthesize" />
  自動統合分析（最賢モデルが全回答を分析）
</label>
```

### 5.2 手動統合分析ボタン

```javascript
// Explore results の下に追加
<button id="synthesizeBtn" onclick="runSynthesis()">
  📊 統合分析を表示
</button>
```

### 5.3 SSEイベント処理

```javascript
eventSource.addEventListener('synthesis_start', (e) => {
  showSynthesisLoader(); // Loading indicator
});

eventSource.addEventListener('synthesis_result', (e) => {
  const data = JSON.parse(e.data);
  addSynthesisCard(data); // 統合分析カードを表示
  updateCredits(data.credits_remaining);
});

eventSource.addEventListener('synthesis_error', (e) => {
  const data = JSON.parse(e.data);
  showError(data.message);
});
```

---

## テスト計画

### 1. 自動統合分析テスト

```bash
curl -X POST http://localhost:3000/api/v1/chat/explore \
  -H "Content-Type: application/json" \
  -d '{
    "message": "Rustとは何ですか？",
    "session_id": "test",
    "auto_synthesize": true
  }'
```

**期待動作**:
1. `explore_result` イベント × N（各モデル）
2. `explore_done` イベント
3. `synthesis_start` イベント
4. `synthesis_result` イベント（Opus/GPT-4o の統合分析）

### 2. 手動統合分析テスト

```bash
curl -X POST http://localhost:3000/api/v1/chat/explore/synthesize \
  -H "Content-Type: application/json" \
  -d '{
    "question": "Rustとは何ですか？",
    "session_id": "test",
    "results": [
      {"model": "claude-sonnet-4-5", "response": "Rustは..."},
      {"model": "gpt-4o", "response": "Rustは..."}
    ]
  }'
```

---

## 期待効果

### Before（現状）
- 複数の回答を読み比べる必要あり
- どれが正しいか判断が難しい

### After（統合分析）
- ✅ 共通見解が自動抽出
- ✅ 相違点が明確化
- ✅ 信頼性評価付き
- ✅ 総合結論が即座に得られる

### コスト
- 自動: 追加1回分のLLMコール（Opus/GPT-4o）
- 手動: ユーザーが必要な時だけ実行

---

## 次のステップ

1. ✅ 実装計画レビュー（このファイル）
2. ⏳ Phase 1: データ構造追加
3. ⏳ Phase 2: 統合分析ロジック実装
4. ⏳ Phase 3: 自動統合分析実装
5. ⏳ Phase 4: 手動統合分析エンドポイント実装
6. ⏳ Phase 5: Web UI 対応
7. ⏳ テスト & デプロイ
