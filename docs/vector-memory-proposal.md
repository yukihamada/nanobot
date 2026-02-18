# Vector Memory Proposal for nanobot

## 現状分析

### 現在のメモリシステム
- **Storage**: DynamoDB (tenant_id + session_key)
- **Content**: Plain text (長期記憶 + デイリーログ)
- **検索**: キーベースのみ（セマンティック検索なし）

### 課題
- 過去の会話を意味的に検索できない
- 関連性の高い記憶を自動取得できない
- 長期記憶が増えるとコンテキスト注入が非効率

---

## 提案1: PostgreSQL pgvector 🔥 推奨

### メリット
- ✅ **低コスト**: Supabase無料枠で始められる（500MB DB）
- ✅ **AWS統合**: RDS PostgreSQL + pgvector拡張で可能
- ✅ **Rust対応**: `sqlx` クレートで型安全なクエリ
- ✅ **高速検索**: HNSW/IVFFlat インデックスでk-NN検索
- ✅ **既存DynamoDB併用**: テキストはDynamoDB、ベクトルはPgに分離

### 実装案

```rust
// Cargo.toml
[features]
vector-memory = ["sqlx", "sqlx-postgres"]

[dependencies]
sqlx = { version = "0.8", features = ["runtime-tokio-rustls", "postgres", "uuid", "chrono"], optional = true }

// memory/pgvector_backend.rs
use sqlx::PgPool;

pub struct PgVectorBackend {
    pool: PgPool,
    user_id: String,
}

impl PgVectorBackend {
    pub async fn new(database_url: &str, user_id: String) -> Result<Self> {
        let pool = PgPool::connect(database_url).await?;
        Ok(Self { pool, user_id })
    }

    /// テキストをベクトル化して保存
    pub async fn store_memory(&self, text: &str, embedding: &[f32]) -> Result<()> {
        sqlx::query!(
            "INSERT INTO memories (user_id, content, embedding, created_at)
             VALUES ($1, $2, $3, NOW())",
            self.user_id,
            text,
            embedding
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// セマンティック検索（コサイン類似度）
    pub async fn search_similar(&self, query_embedding: &[f32], limit: i64) -> Result<Vec<String>> {
        let results = sqlx::query!(
            "SELECT content, 1 - (embedding <=> $1) as similarity
             FROM memories
             WHERE user_id = $2
             ORDER BY embedding <=> $1
             LIMIT $3",
            query_embedding,
            self.user_id,
            limit
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(results.into_iter().map(|r| r.content).collect())
    }
}
```

### PostgreSQL スキーマ

```sql
-- pgvector拡張を有効化
CREATE EXTENSION IF NOT EXISTS vector;

-- メモリテーブル
CREATE TABLE memories (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id TEXT NOT NULL,
    content TEXT NOT NULL,
    embedding vector(1536),  -- OpenAI text-embedding-3-small
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    metadata JSONB DEFAULT '{}'::jsonb
);

-- ベクトル検索用インデックス（HNSW）
CREATE INDEX ON memories USING hnsw (embedding vector_cosine_ops);

-- ユーザー別インデックス
CREATE INDEX ON memories (user_id, created_at DESC);
```

### 埋め込み生成

```rust
// provider/embeddings.rs
pub async fn generate_embedding(text: &str) -> Result<Vec<f32>> {
    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.openai.com/v1/embeddings")
        .json(&serde_json::json!({
            "input": text,
            "model": "text-embedding-3-small"
        }))
        .send()
        .await?;

    let data: EmbeddingResponse = resp.json().await?;
    Ok(data.data[0].embedding.clone())
}
```

### 統合フロー

1. **会話保存時**: テキスト → DynamoDB + ベクトル → PostgreSQL
2. **検索時**: クエリ → ベクトル化 → PgVector検索 → 関連記憶取得
3. **コンテキスト構築**: 長期記憶(text) + セマンティック検索結果(top 5)

---

## 提案2: AWS OpenSearch Serverless

### メリット
- ✅ **AWS ネイティブ**: Lambdaと同一VPC、IAM認証
- ✅ **スケーラブル**: 自動スケーリング
- ✅ **k-NN検索**: faiss/nmslib エンジン内蔵

### デメリット
- ❌ **コスト高め**: $700/月〜（OCU課金）
- ❌ **複雑**: インデックス管理、マッピング設定

### 実装案

```rust
// Cargo.toml
[dependencies]
opensearch = { version = "2", features = ["aws-auth"] }

// memory/opensearch_backend.rs
use opensearch::{OpenSearch, IndexParts};

pub struct OpenSearchBackend {
    client: OpenSearch,
    index: String,
    user_id: String,
}

impl OpenSearchBackend {
    pub async fn search_knn(&self, vector: &[f32], k: usize) -> Result<Vec<String>> {
        let body = serde_json::json!({
            "query": {
                "knn": {
                    "embedding": {
                        "vector": vector,
                        "k": k
                    }
                }
            },
            "filter": {
                "term": { "user_id": self.user_id }
            }
        });

        let resp = self.client
            .search(SearchParts::Index(&[&self.index]))
            .body(body)
            .send()
            .await?;

        // Parse hits...
        Ok(vec![])
    }
}
```

---

## 提案3: クライアント側ベクトル検索（最小構成）

### メリット
- ✅ **追加インフラ不要**: DynamoDB単体
- ✅ **実装シンプル**: Rust の ndarray + 類似度計算

### デメリット
- ❌ **スケールしない**: 全ベクトル取得して計算（100件超で遅い）
- ❌ **Lambda制約**: メモリ/タイムアウトに注意

### 実装案

```rust
// Cargo.toml
[dependencies]
ndarray = "0.16"

// memory/client_vector.rs
use ndarray::{Array1, ArrayView1};

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let a = Array1::from_vec(a.to_vec());
    let b = Array1::from_vec(b.to_vec());

    let dot = a.dot(&b);
    let norm_a = a.dot(&a).sqrt();
    let norm_b = b.dot(&b).sqrt();

    dot / (norm_a * norm_b)
}

impl DynamoMemoryBackend {
    pub async fn search_semantic(&self, query_embedding: &[f32], top_k: usize) -> Result<Vec<String>> {
        // 1. 全記憶を取得（DynamoDB Query）
        let memories = self.get_all_memories().await?;

        // 2. 類似度計算
        let mut scored: Vec<(f32, String)> = memories
            .into_iter()
            .map(|(text, embedding)| {
                let score = cosine_similarity(query_embedding, &embedding);
                (score, text)
            })
            .collect();

        // 3. ソート
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());

        Ok(scored.into_iter().take(top_k).map(|(_, text)| text).collect())
    }
}
```

---

## 推奨実装ロードマップ

### Phase 1: 最小構成（1-2日）
- [ ] DynamoDBにベクトル列追加（Binary型）
- [ ] OpenAI embeddings API統合
- [ ] クライアント側類似度計算（ndarray）
- [ ] `/api/v1/memory/search` エンドポイント

### Phase 2: PostgreSQL統合（3-5日）
- [ ] Supabase/RDSセットアップ
- [ ] pgvector拡張インストール
- [ ] sqlx統合 + マイグレーション
- [ ] DynamoDB → PostgreSQL バッチ移行スクリプト

### Phase 3: 自動記憶統合（1週間）
- [ ] 会話終了時に自動ベクトル化
- [ ] チャット開始時にセマンティック検索
- [ ] デイリーログ要約 + ベクトル化
- [ ] UI: "関連する過去の会話" サジェスト

---

## コスト試算

| 方式 | 初期費用 | 月額コスト（1万ユーザー） |
|------|---------|-------------------------|
| **PostgreSQL (Supabase)** | $0 | $25（Pro plan） |
| **PostgreSQL (RDS)** | $0 | $50-100（db.t4g.micro） |
| **OpenSearch Serverless** | $0 | $700+（OCU課金） |
| **クライアント側** | $0 | $0（DynamoDB費用のみ） |

---

## 次のステップ

どのアプローチにしますか？

1. **PostgreSQL pgvector** — 推奨（低コスト、高性能、スケーラブル）
2. **OpenSearch Serverless** — 大規模向け（将来1M+ users）
3. **クライアント側** — 最小構成（プロトタイプ）

決定したら実装プランを作成します。
