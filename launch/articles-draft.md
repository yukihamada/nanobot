# Launch Articles

---

## Dev.to Article (English)

**Title**: I Built a Multi-Model AI Agent Platform in Rust -- Here's What I Learned

**Tags**: rust, ai, webdev, opensource

**Cover image**: Architecture diagram of teai.io (Rust + Lambda + multi-model)

---

Last year, I was juggling four different AI API keys -- OpenAI, Anthropic, Google, and DeepSeek. Each had its own SDK, authentication flow, rate limits, and error handling. I wanted one interface to rule them all: send a message, get the best response, and let the platform handle model selection, tool calling, and failover.

So I built **nanobot** -- a multi-model AI agent platform written entirely in Rust. It powers two products:

- **[teai.io](https://teai.io)** -- Developer-focused AI agent platform with REST API, multi-model orchestration, and tool calling
- **[chatweb.ai](https://chatweb.ai)** -- Consumer AI assistant with voice-first UX across LINE, Telegram, and Web

Both run on a single AWS Lambda function, serve 14+ channels, and handle everything from web search to voice synthesis. Here's what I learned building it.

### Architecture: One Lambda to Serve Them All

The core architecture is deceptively simple:

```
Client Request
  -> API Gateway
    -> Lambda (Rust binary, ARM64)
      -> axum Router
        -> /api/v1/chat     (REST API)
        -> /webhook/line     (LINE integration)
        -> /webhook/telegram (Telegram integration)
        -> /oauth/callback   (OAuth flows)
        -> /                 (Web UI -- served via include_str!())
      -> DynamoDB (sessions, memory, user data)
      -> AI Providers (Claude, GPT-4o, Gemini, DeepSeek)
```

One Rust binary. One Lambda function. One deployment. The web UI HTML is compiled into the binary with `include_str!()`, so there's no S3 bucket or CDN for the frontend. The entire platform -- API, webhooks, OAuth, web UI -- is a single `cargo build` away.

This approach has trade-offs. You lose independent scaling and deployment of services. But for a solo developer, the operational simplicity is worth it. I deploy once, and everything updates atomically. There's one set of logs, one set of metrics, one thing to debug.

### The LoadBalancedProvider

The multi-model layer is handled by a `LoadBalancedProvider` that wraps all AI providers behind a unified interface:

```rust
pub struct LoadBalancedProvider {
    providers: Vec<Box<dyn AiProvider>>,
    current_index: AtomicUsize,
}

impl LoadBalancedProvider {
    pub async fn chat(
        &self,
        messages: &[Message],
        tools: Option<&[Tool]>,
        tool_choice: Option<&str>,
    ) -> Result<ChatResponse> {
        let start = self.current_index.load(Ordering::Relaxed);
        let len = self.providers.len();

        for i in 0..len {
            let idx = (start + i) % len;
            match self.providers[idx].chat(messages, tools, tool_choice).await {
                Ok(response) => {
                    // Advance round-robin index on success
                    self.current_index.store((idx + 1) % len, Ordering::Relaxed);
                    return Ok(response);
                }
                Err(e) => {
                    log::warn!("Provider {} failed: {}, trying next", idx, e);
                    continue;
                }
            }
        }

        Err(anyhow!("All providers failed"))
    }
}
```

Round-robin distribution with automatic failover. If OpenAI is down, the request silently falls through to Anthropic. If that fails, it tries Google. The caller never knows. This has saved us from multiple provider outages without any manual intervention.

For channel-specific optimization, we override the model selection. Web users get the smartest available model (currently Claude Sonnet). LINE users get a fast model with instructions to keep responses under 200 characters with emoji. Telegram users get Markdown-formatted responses up to 300 characters.

### MCP Tool Integration

One of the most interesting parts of the platform is the tool-calling flow. The AI agent can search the web, check the weather, do calculations, and fetch web pages -- all within a single conversation turn.

The flow uses a three-phase approach:

```rust
// Phase 1: Force tool usage
let response = provider.chat(
    &messages,
    Some(&available_tools),
    Some("required"),  // Forces the model to call a tool
).await?;

// Phase 2: Process tool results, let model decide next step
messages.push(tool_result_message);
let response = provider.chat(
    &messages,
    Some(&available_tools),
    Some("auto"),  // Model decides whether to call more tools
).await?;

// Phase 3: Generate final text response
let response = provider.chat(
    &messages,
    None,   // No tools available -- forces text generation
    None,
).await?;
```

Why three phases? Because LLMs are lazy. If you give them tools with `auto` mode from the start, they'll often skip the tools and give a generic response like "I don't have access to real-time information." By forcing tool usage in the first call, you ensure the agent actually uses its capabilities. The final call with no tools forces it to synthesize everything into a clean text response.

### The Web Search Problem

Here's something nobody tells you about building AI agents on cloud infrastructure: **Google, Bing, DuckDuckGo, and Amazon all block cloud IP ranges.** Your Lambda function will get CAPTCHAs, 403s, or 503s when trying to search the web.

We tried everything:

- Direct HTTP requests to Google: CAPTCHA
- Bing API: works, but expensive at scale
- DuckDuckGo HTML: blocked from AWS IPs
- Amazon search: 503

The solution? **Jina Reader** (`https://r.jina.ai/{url}`). It's a service that fetches and renders web pages, returning clean Markdown. It works from cloud IPs because it routes through its own infrastructure. We use a two-step approach:

1. Search via a search API to get URLs
2. Fetch individual result pages through Jina Reader for content extraction

Not elegant, but reliable.

### Performance: Why Rust on Lambda

Numbers that matter:

| Metric | Value |
|--------|-------|
| Cold start | <200ms |
| Warm response (p50) | <2s |
| Memory usage | ~128MB |
| Binary size | ~15MB (compressed) |
| Build time | ~3 min (release, cross-compile) |

For comparison, a Python Lambda with similar functionality would have 2-5 second cold starts and 256-512MB memory usage. Rust's zero-cost abstractions and lack of a garbage collector make a real difference on Lambda's pay-per-millisecond pricing.

Cross-compiling for Lambda ARM64 from macOS required some setup:

```bash
RUSTUP_TOOLCHAIN=stable \
RUSTC=$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin/rustc \
cargo zigbuild \
  --manifest-path crates/nanobot-lambda/Cargo.toml \
  --release \
  --target aarch64-unknown-linux-gnu
```

`cargo-zigbuild` uses Zig as a cross-compilation linker, which handles the glibc compatibility issues that plague Rust-to-Linux cross-compilation from macOS.

### Lessons Learned the Hard Way

**1. DynamoDB + async Rust = footgun.**

Tokio's runtime doesn't let you call `block_on` from within an async context. When we needed synchronous DynamoDB access from certain code paths, `std::thread::scope` didn't help -- the closure runs on the calling thread, which is still inside Tokio. The fix:

```rust
// WRONG: panics with "Cannot start a runtime from within a runtime"
let result = tokio::runtime::Runtime::new()?.block_on(dynamo_call());

// WRONG: scope runs on current thread, still inside Tokio
std::thread::scope(|s| {
    s.spawn(|| { block_on(dynamo_call()) });
});

// RIGHT: new thread escapes Tokio runtime
let handle = std::thread::spawn(move || {
    tokio::runtime::Runtime::new().unwrap().block_on(dynamo_call())
});
let result = handle.join().unwrap()?;
```

**2. `include_str!()` caches aggressively.**

The web UI HTML is compiled into the binary. Change the HTML, run `cargo build`, and... nothing changes. The compiler sees the Rust source hasn't changed and skips recompilation. You need to `touch` the source file that contains `include_str!()` or do a clean build. This burned hours of debugging before I figured it out.

**3. CORS with multiple domains is tricky.**

The same Lambda serves `chatweb.ai` (web UI) and `api.chatweb.ai` (API). Cross-origin requests from the web UI to the API subdomain require proper CORS headers. The simpler solution: use relative URLs on the web UI so requests go to the same origin. No CORS needed.

**4. OpenAI model name normalization.**

When you configure models as `openai/gpt-4o` internally (to distinguish providers), you must strip the `openai/` prefix before sending the request to the actual OpenAI API. This seems obvious in retrospect, but it caused silent failures because OpenAI returns a generic error for unknown model names.

### Try It

[![GitHub stars](https://img.shields.io/github/stars/yukihamada/nanobot?style=social)](https://github.com/yukihamada/nanobot) [![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://github.com/yukihamada/nanobot/blob/main/LICENSE)

| Product | Use Case | Link |
|---------|----------|------|
| **teai.io** | Developer API -- build AI agents with REST endpoints | [teai.io](https://teai.io) |
| **chatweb.ai** | Consumer assistant -- voice-first AI on LINE, Telegram, Web | [chatweb.ai](https://chatweb.ai) |
| **GitHub** | Source code (MIT license) | [yukihamada/nanobot](https://github.com/yukihamada/nanobot) |

Quick test:

```bash
curl -X POST https://teai.io/api/v1/chat \
  -H "Content-Type: application/json" \
  -d '{"message": "Write a Rust function for binary search"}'
```

Free tier: 1,000 credits/month, no credit card required.

If you're building AI agents in Rust, I'd love to compare notes. The ecosystem is still young, and there's a lot of uncharted territory around tool calling, memory management, and multi-model orchestration. Open an issue on GitHub or find me on X [@yukihamada](https://x.com/yukihamada).

---
---

## Qiita Article (Japanese)

**Title**: Rustで作ったマルチモデルAIエージェント基盤 -- Lambda1つで14チャネル対応

**Tags**: Rust, AI, AWS, Lambda, LINE

[![GitHub stars](https://img.shields.io/github/stars/yukihamada/nanobot?style=social)](https://github.com/yukihamada/nanobot) [![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://github.com/yukihamada/nanobot/blob/main/LICENSE) [![Rust](https://img.shields.io/badge/Built_with-Rust-dea584?logo=rust&logoColor=white)](https://www.rust-lang.org/)

---

### はじめに

**nanobot** というAIエージェント基盤をRustで開発し、2つのプロダクトとして展開しています:

- **[chatweb.ai](https://chatweb.ai)** -- 音声ファーストのAIアシスタント（LINE/Telegram/Web対応、日本語最優先）
- **[teai.io](https://teai.io)** -- 開発者向けAIエージェントプラットフォーム（REST API、マルチモデル、ツール実行）

なぜRustでAIプラットフォームを作ったのか。理由はシンプルです:

1. **Lambda のコールドスタート**: Python だと 2-5 秒。Rust なら 200ms 以下
2. **メモリ効率**: 128MB で十分動く。Python だと 256-512MB 必要
3. **型安全性**: 14 チャネル分のWebhookハンドラを安全に書ける
4. **単一バイナリ**: デプロイが `aws lambda update-function-code` 一発

OpenAI、Anthropic、Google、DeepSeek の4社のAPIキーを管理し、それぞれのSDK、認証、エラーハンドリングを書くのに疲れました。1つのインターフェースで全モデルにアクセスできる基盤が欲しかった。それが nanobot プロジェクトの始まりです。

### アーキテクチャ

全体像はこうなっています:

```
クライアント
  -> API Gateway (api.chatweb.ai / teai.io)
    -> Lambda (Rust バイナリ, ARM64)
      -> axum Router
        -> /api/v1/chat       (REST API)
        -> /webhook/line      (LINE Webhook)
        -> /webhook/telegram  (Telegram Webhook)
        -> /oauth/callback    (OAuth フロー)
        -> /                  (Web UI -- include_str!() でバイナリに埋め込み)
      -> DynamoDB (セッション, メモリ, ユーザーデータ)
      -> AI Providers (Claude, GPT-4o, Gemini, DeepSeek)
```

ポイントは **1つの Lambda 関数で全てを処理している** ことです。Web UI の HTML すら `include_str!()` でバイナリに埋め込んでいます。S3 もCDN も不要。`cargo build` 一発でフロントエンドもバックエンドもまとめてデプロイできます。

マイクロサービスのベストプラクティスには反しますが、個人開発ではオペレーションの簡素さが圧倒的に重要です。ログは1箇所、デプロイは1回、デバッグ対象は1つのバイナリ。

クロスコンパイルは `cargo-zigbuild` を使っています:

```bash
RUSTUP_TOOLCHAIN=stable \
RUSTC=$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin/rustc \
cargo zigbuild \
  --manifest-path crates/nanobot-lambda/Cargo.toml \
  --release \
  --target aarch64-unknown-linux-gnu
```

### マルチモデル対応

`LoadBalancedProvider` がマルチモデルの中核です:

```rust
pub struct LoadBalancedProvider {
    providers: Vec<Box<dyn AiProvider>>,
    current_index: AtomicUsize, // ラウンドロビン用インデックス
}

impl LoadBalancedProvider {
    pub async fn chat(
        &self,
        messages: &[Message],
        tools: Option<&[Tool]>,
        tool_choice: Option<&str>,
    ) -> Result<ChatResponse> {
        let start = self.current_index.load(Ordering::Relaxed);
        let len = self.providers.len();

        // 全プロバイダーを順番に試す(フェイルオーバー)
        for i in 0..len {
            let idx = (start + i) % len;
            match self.providers[idx].chat(messages, tools, tool_choice).await {
                Ok(response) => {
                    // 成功したら次回のために index を進める
                    self.current_index.store((idx + 1) % len, Ordering::Relaxed);
                    return Ok(response);
                }
                Err(e) => {
                    log::warn!("Provider {} failed: {}, trying next", idx, e);
                    continue; // 次のプロバイダーへ
                }
            }
        }

        Err(anyhow!("全プロバイダーが失敗しました"))
    }
}
```

ラウンドロビンでリクエストを分散し、失敗したら自動的に次のプロバイダーにフォールバックします。OpenAI が落ちても Anthropic に自動切り替え。呼び出し側はプロバイダーの障害を意識する必要がありません。

### ツール呼び出し

web_search、calculator、weather、web_fetch のツールをチャットフロー内で使えます。ツール呼び出しは3フェーズで制御しています:

```rust
// フェーズ1: ツール使用を強制
// LLM は「リアルタイム情報がありません」と言いがち。強制することで確実にツールを使わせる
let response = provider.chat(
    &messages,
    Some(&tools),
    Some("required"),  // OpenAI: "required" / Anthropic: {"type":"any"}
).await?;

// フェーズ2: ツール結果を渡し、追加のツール呼び出しを判断させる
messages.push(tool_result);
let response = provider.chat(
    &messages,
    Some(&tools),
    Some("auto"),  // モデルが判断
).await?;

// フェーズ3: ツールを渡さず、テキスト生成を強制
let response = provider.chat(
    &messages,
    None,   // ツールなし -> テキスト生成を強制
    None,
).await?;
```

`tool_choice` の使い分けがキモです:
- `"required"` (OpenAI) / `{"type":"any"}` (Anthropic): 最初の呼び出しでツール使用を強制
- `"auto"` (OpenAI) / `{"type":"auto"}` (Anthropic): 2回目以降、モデルに判断を委ねる
- ツールを `None` にする: 最終呼び出しでテキスト生成を強制

### チャネル別最適化

同じ Lambda でも、チャネルによって最適な応答は異なります:

| チャネル | モデル | 文字数上限 | フォーマット |
|---------|--------|-----------|------------|
| LINE | 高速モデル | 200字 | 絵文字あり |
| Telegram | 標準モデル | 300字 | Markdown |
| Web | 最賢モデル (Claude Sonnet) | 制限なし | リッチHTML |

```rust
// チャネルに応じたシステムプロンプトの調整
let system_prompt = match channel {
    Channel::Line => "200字以内で回答。適度に絵文字を使用。",
    Channel::Telegram => "300字以内でMarkdown形式で回答。",
    Channel::Web => "詳細に回答。コードブロック、リスト等を活用。",
};
```

LINE ユーザーは通勤中にサッと質問したい。Web ユーザーはじっくり深い回答が欲しい。チャネルを検出して自動的に応答スタイルを切り替えることで、どこでも自然な体験を提供しています。

### 音声対応

chatweb.ai の特徴は音声ファーストであることです:

- **STT (音声認識)**: Web Speech API (ja-JP) を使用。ブラウザ側で完結するためサーバー負荷ゼロ
- **TTS (音声合成)**: OpenAI tts-1 (nova voice) を使用。`POST /api/v1/speech/synthesize` エンドポイント。100文字あたり1クレジット

Push-to-talk ボタンを押して話すと、音声がテキストに変換され、AIの応答が音声で返ってきます。料理中やジョギング中にAIと会話できる体験を目指しました。

### LINE 連携のユースケース

日本のユーザーにとって LINE は生活インフラです。chatweb.ai の LINE Bot（[@619jcqqh](https://line.me/R/ti/p/@619jcqqh)）では以下のような使い方ができます:

- **通勤中の情報収集**: 「今日の為替は？」→ Web検索ツールで最新データを取得、200字以内で回答
- **買い物リスト**: 「冷蔵庫にあるものでカレーの材料リスト作って」→ 前回の会話を記憶しているので文脈を理解
- **翻訳**: 英語のメールをそのまま転送 → 自然な日本語に翻訳して返答
- **チャネル連携**: Web で始めた会話を LINE で続行。`/link` コマンドで QR コード不要の即時連携

```
# LINE で /link を送信 → 6桁コードが返る
# Web チャットで /link ABC123 を送信 → 連携完了
# 以降、同じ会話がどちらからでもアクセス可能
```

LINE の200字制限に最適化された応答を返すため、同じ質問でもWeb版とは異なるフォーマットで回答します。絵文字を適度に使い、箇条書きで簡潔に。

### ハマりポイント

Rust で AI プラットフォームを作る際に踏んだ地雷をまとめます。

**1. DynamoDB の async Rust での block_on 問題**

Tokio の async ランタイム内で `block_on` を呼ぶとパニックします。`std::thread::scope` も罠で、クロージャは現在のスレッド(Tokio ランタイム内)で実行されます:

```rust
// NG: "Cannot start a runtime from within a runtime" でパニック
let rt = tokio::runtime::Runtime::new()?;
let result = rt.block_on(dynamo_call());

// NG: scope は新しいスレッドを作らない。現在のスレッドで実行される
std::thread::scope(|s| {
    s.spawn(|| { block_on(dynamo_call()) });
});

// OK: std::thread::spawn で Tokio ランタイム外のスレッドを作る
let handle = std::thread::spawn(move || {
    tokio::runtime::Runtime::new().unwrap().block_on(dynamo_call())
});
let result = handle.join().unwrap()?;
```

**2. Web検索のクラウドIPブロック**

Google、Bing、DuckDuckGo、Amazon -- 全て AWS の IP レンジからのアクセスをブロックまたは CAPTCHA で弾きます。解決策は [Jina Reader](https://r.jina.ai/) です:

```rust
// Jina Reader 経由で Web ページを取得
let url = format!("https://r.jina.ai/{}", target_url);
let response = reqwest::get(&url).await?;
let markdown = response.text().await?; // クリーンな Markdown が返る
```

Jina Reader は JavaScript のレンダリングも行ってくれるため、SPA のページも取得できます。

**3. include_str!() のビルドキャッシュ問題**

HTML を `include_str!()` で埋め込んでいますが、HTML を変更しても `cargo build` が変更を検知しないことがあります。Rust のソースファイルが変わっていないため、再コンパイルがスキップされます。

対策: `include_str!()` を含むソースファイルを `touch` するか、`cargo clean` してからビルドします。Cargo.toml の `build.rs` で `println!("cargo:rerun-if-changed=static/index.html")` を追加するのがベストプラクティスです。

**4. OpenAI のモデル名正規化**

内部的にモデルを `openai/gpt-4o` と管理していますが、OpenAI API に送る際は `openai/` プレフィックスを除去する必要があります。除去し忘れると、OpenAI はモデル不明のエラーを返します。`normalize_model()` 関数で統一的に処理しています。

### まとめ

Rust x Lambda は AI プラットフォームに最適な組み合わせです:

- **高速**: コールドスタート 200ms 以下、レスポンス 2 秒以下
- **省メモリ**: 128MB で十分
- **型安全**: 14 チャネル分の Webhook を安全に処理
- **単一バイナリ**: デプロイ・運用が圧倒的にシンプル

もちろんトレードオフはあります。コンパイル時間は長い (リリースビルドで約3分)。エコシステムは Python に比べて未成熟。でも、本番環境での安定性とパフォーマンスは、開発時の不便さを補って余りあります。

試してみてください:

| プロダクト | 用途 | リンク |
|-----------|------|--------|
| **teai.io** | 開発者向け API -- マルチモデル、ツール実行、SSEストリーミング | [teai.io](https://teai.io) |
| **chatweb.ai** | 音声ファースト AI -- LINE、Telegram、Web で使える | [chatweb.ai](https://chatweb.ai) |
| **LINE Bot** | 友だち追加して即利用可能 | [@619jcqqh](https://line.me/R/ti/p/@619jcqqh) |
| **GitHub** | ソースコード（MIT ライセンス） | [yukihamada/nanobot](https://github.com/yukihamada/nanobot) |

```bash
# API を試す
curl -X POST https://teai.io/api/v1/chat \
  -H "Content-Type: application/json" \
  -d '{"message": "Rustでバイナリサーチを実装して"}'
```

フリープランで月1,000クレジット使えます。クレジットカード不要。

質問やフィードバックがあれば、GitHub Issue か X ([@yukihamada](https://x.com/yukihamada)) までお気軽にどうぞ。
