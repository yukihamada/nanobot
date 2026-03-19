---
title: "日本発のLLM API Gateway「teai.io」のアーキテクチャを全公開"
emoji: "🏗"
type: "tech"
topics: ["Rust", "AWS", "LLM", "アーキテクチャ", "API"]
published: false
---

# 日本発のLLM API Gateway「teai.io」のアーキテクチャを全公開

## はじめに

[teai.io](https://teai.io) は、日本のAI開発者向けに設計されたLLM API Gatewayです。45以上のモデルをOpenAI互換のAPIで提供し、東京リージョンで低レイテンシを実現しています。

この記事では、teai.ioの技術的アーキテクチャを公開します。LLM Gatewayを自分で構築したい方や、似たようなサービスのアーキテクチャに興味がある方の参考になれば嬉しいです。

## 全体アーキテクチャ

```
                          ┌─────────────────────┐
                          │   Cloudflare DNS     │
                          │   + CF Workers       │
                          │   (teai-edge)        │
                          └──────────┬───────────┘
                                     │
                          ┌──────────▼───────────┐
                   ┌──────│   AWS API Gateway     │
                   │      │   (ap-northeast-1)    │
                   │      └──────────┬───────────┘
                   │                 │
          fallback │      ┌──────────▼───────────┐
                   │      │   AWS Lambda (ARM64)  │
                   │      │   Rust (axum)         │
                   │      │   "nanobot-prod"      │
                   │      └──────────┬───────────┘
                   │                 │
          ┌────────▼──┐    ┌────────▼────────┐
          │  Fly.io   │    │    DynamoDB     │
          │  (nrt)    │    │  (sessions,    │
          │  fallback │    │   users, etc.) │
          └───────────┘    └────────┬────────┘
                                    │
                          ┌─────────▼─────────┐
                          │  LLM Providers     │
                          │  ┌───┐ ┌───┐ ┌───┐│
                          │  │OAI│ │ANT│ │GGL││
                          │  └───┘ └───┘ └───┘│
                          │  ┌───┐ ┌───┐ ┌───┐│
                          │  │GRQ│ │RPD│ │DS ││
                          │  └───┘ └───┘ └───┘│
                          └───────────────────┘
```

## なぜRust + Lambda？

### 選定理由

1. **コールドスタート**: Rustバイナリは~5MBに収まり、Lambda上のコールドスタートが**50-100ms**。Node.js/Pythonの300-1000msと比べて圧倒的に速い
2. **メモリ効率**: 128MBのLambdaで十分動作（実際は256MB設定）。ランタイムコストが安い
3. **型安全性**: 45+モデルのルーティング、クレジット計算、認証を型で保証
4. **axum**: tokioベースの非同期Webフレームワーク。SSEストリーミングとの相性が抜群

### ビルドの罠

Lambda (AL2023) で動かすにはmuslターゲットが**必須**です。gnuだとglibc互換性で `Runtime.ExitError` になります。

```bash
# 正しい
cargo zigbuild --target aarch64-unknown-linux-musl --release

# NG: 絶対にやらない
# cargo build --target aarch64-unknown-linux-gnu
```

> これは実際に踏んだ地雷で、デバッグに半日かかりました。Lambda + Rustの組み合わせでは必ずmuslを使ってください。

## エッジ層: Cloudflare Workers

```javascript
// teai-edge Worker（簡略版）
export default {
  async fetch(request, env) {
    const url = new URL(request.url);

    // ヘルスチェック
    if (url.pathname === "/health") {
      return new Response("ok");
    }

    // プライマリ: AWS Lambda
    try {
      const response = await fetch(env.PRIMARY_BACKEND + url.pathname, {
        method: request.method,
        headers: {
          ...Object.fromEntries(request.headers),
          "X-Forwarded-Host": url.hostname,  // ブランド判定用
        },
        body: request.body,
      });

      if (response.ok) return response;
    } catch (e) {
      // フォールバックへ
    }

    // フォールバック: Fly.io
    return fetch(env.FALLBACK_BACKEND + url.pathname, {
      method: request.method,
      headers: request.headers,
      body: request.body,
    });
  }
};
```

### なぜCF Workers？

- **レイテンシ**: CDNエッジでDNS解決〜TLS終端が完了。東京PoPから直接Lambdaへ
- **フォールバック**: Lambdaが落ちてもFly.ioに自動切替
- **コスト**: 無料プランで10万リクエスト/日。現状のトラフィックでは$0

## LLMプロバイダルーティング

teai.ioの核心は**プロバイダルーティング**です。45+モデルを、それぞれ最適なプロバイダに振り分けます。

```rust
// モデル名からプロバイダを決定（簡略版）
fn route_model(model: &str) -> Provider {
    match model {
        m if m.starts_with("gpt-") => Provider::OpenAI,
        m if m.starts_with("claude-") => Provider::Anthropic,
        m if m.starts_with("gemini-") => Provider::Google,
        m if m.contains("nemotron") => Provider::RunPod,
        m if m.starts_with("deepseek") => Provider::DeepSeek,
        m if m.contains("qwen") => Provider::Groq, // Groq推論が最速
        _ => Provider::OpenAI, // デフォルト
    }
}
```

### フォールバックチェーン

各モデルには代替プロバイダが設定されています：

```rust
fn fallback_chain(model: &str) -> Vec<Provider> {
    match model {
        "gpt-4o" => vec![
            Provider::OpenAI,
            Provider::OpenRouter,  // OpenAI障害時
        ],
        "claude-sonnet-4-6" => vec![
            Provider::Anthropic,
            Provider::OpenRouter,
        ],
        "nemotron-9b" => vec![
            Provider::RunPod,     // 自前GPU
            Provider::Groq,       // フォールバック
        ],
        _ => vec![Provider::OpenAI],
    }
}
```

## SSEストリーミング

LLM APIの多くはServer-Sent Events (SSE) でストリーミングレスポンスを返します。teai.ioはこれを**透過的に中継**します。

```rust
async fn handle_streaming(
    provider_response: Response<Body>,
) -> impl IntoResponse {
    let stream = provider_response
        .into_body()
        .into_data_stream()
        .map(|chunk| {
            // プロバイダ固有のフォーマットをOpenAI互換に変換
            let data = transform_to_openai_format(chunk?);
            Ok::<_, Error>(Event::default().data(data))
        });

    Sse::new(stream)
        .keep_alive(KeepAlive::default().interval(Duration::from_secs(15)))
}
```

### Anthropic → OpenAI変換

Claudeは独自のSSEフォーマットを使うため、リアルタイムで変換します：

```
// Anthropic形式
event: content_block_delta
data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"Hello"}}

// → OpenAI互換形式に変換
data: {"choices":[{"delta":{"content":"Hello"},"index":0}]}
```

## クレジットシステム

### 設計思想

- **透過的**: プロバイダの原価がそのまま見える
- **シンプル**: 1クレジット ≈ 特定のトークン数（モデルにより異なる）
- **リアルタイム**: レスポンスヘッダ `X-Credits-Remaining` で残高確認

```rust
struct ModelPricing {
    credits_per_1k_input: f64,
    credits_per_1k_output: f64,
}

fn calculate_cost(model: &str, input_tokens: u64, output_tokens: u64) -> f64 {
    let pricing = get_pricing(model);
    let input_cost = (input_tokens as f64 / 1000.0) * pricing.credits_per_1k_input;
    let output_cost = (output_tokens as f64 / 1000.0) * pricing.credits_per_1k_output;
    input_cost + output_cost
}
```

## ブランド分離

teai.ioとchatweb.aiは同じLambdaで動作し、`Host`ヘッダーでブランドを判定します。

```rust
fn effective_host(headers: &HeaderMap) -> String {
    headers.get("x-forwarded-host")
        .or_else(|| headers.get("host"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("chatweb.ai")
        .to_string()
}

fn is_teai(host: &str) -> bool {
    host.contains("teai.io")
}
```

この設計により、インフラコストを共有しつつ、完全に異なるブランド体験を提供できます。

## パフォーマンス

実測値（東京→東京、2026年3月時点）：

| メトリクス | 値 |
|-----------|-----|
| DNS解決 | ~5ms (CF) |
| TLS接続 | ~10ms (CF edge) |
| Edge → Lambda | ~15ms |
| Lambda コールドスタート | ~80ms |
| Lambda ウォームスタート | ~5ms |
| **合計オーバーヘッド** | **~35-110ms** |

> LLMの推論自体が500ms-5sかかるため、35-110msのオーバーヘッドはほぼ無視できます。

## インフラコスト

月間1万リクエスト想定：

| コンポーネント | 月額 |
|---------------|------|
| CF Workers | $0（無料枠内） |
| AWS Lambda (256MB) | ~$0.50 |
| API Gateway | ~$3.50 |
| DynamoDB | ~$1.00 |
| **合計** | **~$5.00** |

> トラフィックが増えても、Lambdaの従量課金のおかげでコストはリニアにスケール。固定費がほぼゼロなのが強み。

## 学んだこと

### 1. musl vs gnu
Lambdaでは**必ずmusl**。gnuは動かない。

### 2. include_str!() のキャッシュ
HTMLをバイナリに埋め込む `include_str!()` は、ソース変更だけでは再ビルドされないことがある。`cargo clean` が必要。

### 3. API Gatewayの$LATEST
API Gatewayは `$LATEST` を直接呼び出す設定。`update-function-code` した瞬間にプロダクション影響。カナリアデプロイが事実上できない。

### 4. SSEのタイムアウト
ストリーミング中のアイドルタイムアウトは30秒。長い推論（O3等）ではkeep-aliveが必須。

## おわりに

teai.ioのアーキテクチャは、**Rust + Lambda + CF Workers** というシンプルな構成で、低コスト・低レイテンシ・高可用性を実現しています。

LLM API Gatewayを自分で構築する際の参考になれば幸いです。そして、もし「自分で作るより使いたい」と思ったら、ぜひ [teai.io](https://teai.io) を試してみてください。

---

**リンク:**
- サイト: [teai.io](https://teai.io)
- API Docs: [teai.io/docs](https://teai.io/docs)
- 登録（無料）: [teai.io/register](https://teai.io/register)
