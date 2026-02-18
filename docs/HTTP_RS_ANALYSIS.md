# HTTP Module Usage Analysis — nanobot/chatweb.ai

**Project**: nanobot (chatweb.ai)
**Module**: `crates/nanobot-core/src/service/http/mod.rs`
**Analysis Date**: 2026-02-15
**Total LOC**: 18,088 lines (http module + handlers)

---

## エグゼクティブサマリー

`service/http/mod.rs` は **3,663行** の大規模モジュールで、以下の3つの責務を持つ：

1. **HTTPルーター定義** — 143エンドポイントのルーティング定義 (`create_router`, `serve`)
2. **共有型定義** — 32個の公開構造体 (Request/Response型、AppState等)
3. **ユーティリティ関数** — 25個のヘルパー関数 (認証、課金、メモリ、監査ログ等)

**結論**: このモジュールは **削除不可** であり、むしろ **分割リファクタリング** が推奨される。

---

## 1. 使用箇所の特定

### 1.1 外部からの使用 (2箇所)

| ファイル | 使用内容 | 目的 |
|---------|---------|------|
| `src/main.rs:553` | `use nanobot_core::service::http::{serve, AppState}` | ローカル開発サーバー起動 |
| `crates/nanobot-lambda/src/main.rs:7` | `use nanobot_core::service::http::{create_router, AppState}` | Lambda関数のエントリーポイント |

### 1.2 内部からの使用 (9ハンドラーファイル)

全ハンドラーが `super::super::{}` で `http/mod.rs` から以下をインポート：

#### auth.rs (1,460行)
```rust
use super::super::{
    AppState, SK_PROFILE,
    RegisterRequest, LoginRequest, EmailAuthRequest, VerifyRequest,
    GoogleCallbackParams, GoogleAuthParams,
};
```
- **依存する型**: 6個
- **使用する関数**: `resolve_session_key`, `get_or_create_user`, `emit_audit_log`, `check_rate_limit`

#### billing.rs (1,218行)
```rust
use super::super::{
    AppState, SK_PROFILE,
    CheckoutRequest, CreditPackRequest, AutoChargeRequest,
};
```
- **依存する型**: 4個
- **使用する関数**: `resolve_session_key`, `get_or_create_user`, `link_stripe_to_user`, `add_credits_to_user`, `find_user_by_stripe_customer`, `find_user_by_email`, `get_base_url`

#### chat.rs (2,477行 — 最大)
```rust
use super::super::{
    AppState, is_admin, SK_PROFILE,
    ChatRequest, ChatResponse, UserSettings,
    URL_REGEX,
    RESPONSE_DEADLINE_SECS, GITHUB_TOOL_NAMES,
    META_INSTRUCTION_JA, META_INSTRUCTION_EN,
};
```
- **依存する型**: 6個
- **使用する関数**: `is_admin`, `check_rate_limit`, `read_memory_context`, `append_daily_memory`, `spawn_consolidate_memory`, `resolve_session_key`, `get_or_create_user`, `deduct_credits`, `spawn_update_conv_meta`
- **依存する定数**: 5個 (URL_REGEX, RESPONSE_DEADLINE_SECS, GITHUB_TOOL_NAMES, META_INSTRUCTION_JA/EN)

#### conversations.rs (609行)
```rust
use super::super::{
    AppState, get_base_url,
};
```
- **依存する型**: 1個
- **使用する関数**: `get_base_url`

#### feedback_loop.rs (878行)
```rust
use super::super::{is_admin, AppState};
```
- **依存する関数**: `is_admin`

#### misc.rs (3,458行 — 2位)
```rust
use super::super::{
    AppState, get_base_url, SK_PROFILE,
    DeviceHeartbeat, WorkerRegisterRequest, WorkerResultRequest, WorkerHeartbeatRequest,
    SyncListParams, SyncPushRequest,
    PartnerGrantCreditsRequest, PartnerVerifySubscriptionRequest, AGENTS,
};
```
- **依存する型**: 9個
- **依存する定数**: 2個 (SK_PROFILE, AGENTS)

#### pages.rs (1,250行)
```rust
use super::super::{
    AppState, is_admin, get_base_url, SK_PROFILE,
    LOCALIZED_PAGES, INDEX_HTML,
};
```
- **依存する定数**: 3個 (SK_PROFILE, LOCALIZED_PAGES, INDEX_HTML)
- **使用する関数**: `is_admin`, `get_base_url`

#### voice.rs (1,749行)
```rust
use super::super::{
    AppState, SK_PROFILE,
};
```
- **依存する定数**: 1個

#### webhooks.rs (1,307行)
```rust
use super::super::{
    AppState,
};
```
- **依存する型**: 1個

---

## 2. 各使用箇所の目的と依存関係

### 2.1 公開API (外部使用)

#### `create_router(state: Arc<AppState>) -> Router`
- **目的**: 143エンドポイントのルーティング定義
- **使用先**: Lambda関数 (`nanobot-lambda/src/main.rs`)
- **依存**: 全ハンドラー関数 (`handlers/*`)
- **削除不可**: Lambda関数のエントリーポイント

#### `serve(addr: &str, state: Arc<AppState>) -> Result<()>`
- **目的**: ローカル開発用HTTPサーバー起動
- **使用先**: ローカル開発 (`src/main.rs`)
- **依存**: `create_router`
- **削除不可**: 開発環境で必須

#### `AppState`
- **目的**: アプリ全体の共有ステート（設定、セッション、LLM、ツール）
- **使用先**: 全エンドポイント
- **依存**: `Config`, `SessionStore`, `LlmProvider`, `ToolRegistry`
- **削除不可**: アーキテクチャの中核

### 2.2 共有型定義 (32個)

#### Request/Response型 (20個)
- `ChatRequest`, `ChatResponse`, `ErrorResponse`
- `RegisterRequest`, `LoginRequest`, `EmailAuthRequest`, `VerifyRequest`
- `CheckoutRequest`, `CreditPackRequest`, `AutoChargeRequest`
- `SyncListParams`, `SyncPushRequest`, etc.

**目的**: APIの型安全性、Serde自動シリアライズ
**依存先**: 全ハンドラー
**削除不可**: APIスキーマの定義

#### 内部型 (12個)
- `UserProfile`, `UserSettings`, `SessionInfo`, `UsageResponse`
- `DeviceHeartbeat`, `WorkerRegisterRequest`, etc.

**目的**: 内部ロジックの型定義
**削除不可**: ドメインモデル

### 2.3 ユーティリティ関数 (25個)

#### 認証・認可
- `is_admin(key: &str) -> bool` — Admin判定 (環境変数)
- `resolve_session_key(dynamo, table, id) -> String` — セッションID→ユーザーキー変換
- `is_session_id(text: &str) -> bool` — セッションID形式判定
- `auto_link_session(...)` — 自動セッションリンク

**使用先**: auth.rs, billing.rs, chat.rs
**削除不可**: 認証基盤

#### 課金・ユーザー管理
- `get_or_create_user(dynamo, table, id) -> UserProfile` — ユーザー取得/作成
- `deduct_credits(dynamo, table, user, amount) -> i64` — クレジット引き落とし
- `add_credits_to_user(dynamo, table, user, amount)` — クレジット付与
- `link_stripe_to_user(...)` — Stripe顧客ID紐付け
- `find_user_by_stripe_customer(...) -> Option<UserProfile>` — Stripe→ユーザー検索
- `find_user_by_email(...) -> Option<UserProfile>` — メール→ユーザー検索

**使用先**: auth.rs, billing.rs, chat.rs
**削除不可**: 課金基盤

#### メモリ管理
- `read_memory_context(dynamo, table, user) -> String` — 長期記憶取得
- `append_daily_memory(dynamo, table, user, text)` — 日次ログ追記
- `spawn_consolidate_memory(dynamo, table, user, date)` — メモリ統合 (非同期)

**使用先**: chat.rs
**削除不可**: 長期記憶機能

#### 監査・ロギング
- `emit_audit_log(dynamo, table, event, user, email, details)` — 監査ログ出力 (fire-and-forget)
- `log_routing_data(...)` — UTM/リファラー記録
- `check_rate_limit(dynamo, table, key, limit) -> bool` — レート制限

**使用先**: auth.rs, billing.rs
**削除不可**: セキュリティ・コンプライアンス

#### その他
- `get_base_url() -> String` — ベースURL取得 (環境変数)
- `spawn_update_conv_meta(...)` — 会話メタデータ更新 (非同期)
- `is_promo_active() -> bool` — プロモーション期間判定

**使用先**: billing.rs, conversations.rs, pages.rs
**削除不可**: URL生成、非同期更新

### 2.4 定数・静的データ (10個)

| 定数名 | 使用先 | 目的 |
|--------|--------|------|
| `SK_PROFILE` | auth, billing, chat, misc, pages, voice | DynamoDB sort key定数 |
| `RESPONSE_DEADLINE_SECS` | chat | タイムアウト時間 (12秒) |
| `GITHUB_TOOL_NAMES` | chat | Admin専用ツール名リスト |
| `URL_REGEX` | chat | URL抽出用正規表現 |
| `META_INSTRUCTION_JA/EN` | chat | メタ認知プロンプト |
| `INDEX_HTML` | pages | トップページHTML (include_str!) |
| `LOCALIZED_PAGES` | pages | ローカライズOGPページ (10言語) |
| `LOCALES` | pages | OGPメタデータ (10言語) |
| `AGENTS` | misc | エージェントプロフィール配列 |
| `PROMO_START` | billing | プロモーション開始日 |

---

## 3. 削除した場合の影響分析

### 3.1 即座に発生する問題

#### ❌ ビルドエラー (100%発生)
- Lambda関数がコンパイル不可 → **本番環境停止**
- ローカル開発サーバー起動不可 → **開発不可**
- 全ハンドラーが依存解決失敗 → **全機能停止**

#### ❌ 型定義の喪失
- 32個の Request/Response 型が消失
- API スキーマが未定義状態に
- Serde シリアライズ/デシリアライズ不可

#### ❌ 共通ロジックの重複
- 25個のユーティリティ関数が消失
- 各ハンドラーで同じロジックを再実装する必要
- コード重複 → 保守性悪化、バグ混入リスク増加

### 3.2 リファクタリング不可避の理由

#### 単一責任の原則違反 (SRP)
`http/mod.rs` は現在、以下の3つの責任を持つ：
1. **ルーティング** — `create_router`, `serve`
2. **型定義** — 32個の構造体
3. **ビジネスロジック** — 25個のヘルパー関数

**理想**: 各責任を独立モジュールに分離

#### コード量の問題
- **3,663行** は単一ファイルとしては過大
- 認知負荷が高く、変更時の影響範囲が不明確
- テストが困難 (単体テスト vs 結合テスト)

---

## 4. リファクタリングオプション

### オプション1: 現状維持 (非推奨)

**概要**: 何もしない

**メリット**:
- 作業コスト: ゼロ
- 既存コードが動作し続ける

**デメリット**:
- コード量が今後も増え続ける (現在 3,663行)
- 保守性が低い (変更時の影響範囲が不明)
- テストが困難 (単体テスト vs 結合テスト)
- 新規メンバーのオンボーディング困難

**推奨度**: ⭐☆☆☆☆ (0/5)

---

### オプション2: 機能を各ハンドラーに分散 (非推奨)

**概要**: `http/mod.rs` の関数を各ハンドラーにコピー

**例**:
```rust
// auth.rs に移動
fn is_admin(key: &str) -> bool { ... }
fn emit_audit_log(...) { ... }

// billing.rs に移動
fn deduct_credits(...) { ... }
fn add_credits_to_user(...) { ... }

// chat.rs に移動
fn read_memory_context(...) { ... }
fn append_daily_memory(...) { ... }
```

**メリット**:
- 各ハンドラーが自己完結的になる
- `super::super::` インポート不要

**デメリット**:
- **コード重複が大量発生** (`get_or_create_user`, `resolve_session_key` 等が複数箇所に)
- **DRY原則違反** (Don't Repeat Yourself)
- バグ修正時に全コピーを修正する必要 → **保守地獄**
- テストコード重複 → テスト保守コスト増
- 共通型 (AppState等) は結局残る

**推奨度**: ⭐☆☆☆☆ (0/5)

---

### オプション3: 新しい抽象化レイヤー作成 (推奨)

**概要**: 責任ごとにモジュール分割

#### 3.1 推奨ディレクトリ構成

```
crates/nanobot-core/src/service/http/
├── mod.rs                  # ルーター定義のみ (create_router, serve) — 200行
├── types.rs                # 全Request/Response型 (32個) — 500行
├── state.rs                # AppState定義 — 100行
├── utils/
│   ├── mod.rs              # 再エクスポート
│   ├── auth.rs             # 認証・認可 (is_admin, resolve_session_key, etc.) — 300行
│   ├── billing.rs          # 課金 (deduct_credits, add_credits_to_user, etc.) — 400行
│   ├── memory.rs           # メモリ (read_memory_context, append_daily_memory, etc.) — 300行
│   └── audit.rs            # 監査ログ (emit_audit_log, log_routing_data, etc.) — 200行
├── constants.rs            # 定数 (SK_PROFILE, GITHUB_TOOL_NAMES, etc.) — 100行
└── handlers/               # 既存のまま
    ├── mod.rs
    ├── auth.rs
    ├── billing.rs
    ├── chat.rs
    ├── conversations.rs
    ├── feedback_loop.rs
    ├── misc.rs
    ├── pages.rs
    ├── voice.rs
    └── webhooks.rs
```

**合計**: 1,800行 (現在 3,663行 → **50%削減**)

#### 3.2 移行後のインポート例

##### Before (現在)
```rust
// handlers/chat.rs
use super::super::{
    AppState, is_admin, SK_PROFILE,
    ChatRequest, ChatResponse, UserSettings,
    URL_REGEX, RESPONSE_DEADLINE_SECS,
    META_INSTRUCTION_JA, META_INSTRUCTION_EN,
};
```

##### After (リファクタ後)
```rust
// handlers/chat.rs
use crate::service::http::{
    state::AppState,
    types::{ChatRequest, ChatResponse, UserSettings},
    utils::auth::{is_admin, resolve_session_key, get_or_create_user},
    utils::billing::deduct_credits,
    utils::memory::{read_memory_context, append_daily_memory},
    constants::{SK_PROFILE, URL_REGEX, RESPONSE_DEADLINE_SECS, META_INSTRUCTION_JA, META_INSTRUCTION_EN},
};
```

**メリット**:
- ✅ **インポート元が明確** (auth/billing/memory のどこから来ているか一目瞭然)
- ✅ **責任が明確** (認証ロジックは `utils::auth` に集約)
- ✅ **テストが容易** (各モジュール単位でテスト可能)
- ✅ **並行開発が可能** (auth.rs と billing.rs を別々に変更してもコンフリクトしにくい)

**デメリット**:
- 初回移行コスト (1-2日)
- インポート文が若干長くなる (明示的になるため許容範囲)

**推奨度**: ⭐⭐⭐⭐⭐ (5/5)

---

### オプション4: ドメイン駆動設計 (DDD) — 将来の理想形

**概要**: ビジネスロジックをドメイン層に分離

```
crates/nanobot-core/src/
├── domain/                 # ドメイン層 (純粋ロジック)
│   ├── user/
│   │   ├── mod.rs
│   │   ├── entity.rs       # UserProfile, UserSettings
│   │   ├── repository.rs   # trait UserRepository
│   │   └── service.rs      # get_or_create_user, deduct_credits
│   ├── session/
│   │   ├── mod.rs
│   │   ├── entity.rs       # Session
│   │   └── repository.rs   # trait SessionRepository
│   └── billing/
│       ├── mod.rs
│       ├── entity.rs       # CreditTransaction
│       └── service.rs      # deduct_credits, add_credits
├── infrastructure/         # インフラ層 (DynamoDB実装)
│   ├── dynamo/
│   │   ├── user_repository.rs   # impl UserRepository for DynamoUserRepo
│   │   └── session_repository.rs
└── service/http/           # プレゼンテーション層 (HTTP API)
    ├── mod.rs              # ルーター
    ├── types.rs            # Request/Response型
    └── handlers/           # コントローラー
```

**メリット**:
- ✅ **テスタビリティ最高** (ドメインロジックが純粋関数に)
- ✅ **DynamoDB以外への移行が容易** (Repository trait を実装するだけ)
- ✅ **ビジネスロジックの再利用** (CLIからもHTTPからも同じドメインサービスを使用)

**デメリット**:
- 初回移行コスト: **大** (1-2週間)
- 過度な抽象化リスク (小規模プロジェクトでは過剰)

**推奨度**: ⭐⭐⭐☆☆ (3/5) — 将来的に検討すべき

---

## 5. 推奨アプローチと実装手順

### 推奨: **オプション3** (抽象化レイヤー分割)

**理由**:
1. **バランスが良い** — 過度な複雑化を避けつつ、保守性を大幅改善
2. **段階的移行が可能** — 全体を止めずに少しずつ移行
3. **テストが書きやすい** — 各モジュール単位でテスト可能
4. **並行開発が可能** — チームで作業分担しやすい

---

### 実装手順 (段階的移行)

#### Phase 1: 基礎構造作成 (1日)

**目的**: 新モジュール作成 + 定数移行

##### ステップ1.1: ディレクトリ作成
```bash
cd crates/nanobot-core/src/service/http/
mkdir utils
touch utils/mod.rs utils/auth.rs utils/billing.rs utils/memory.rs utils/audit.rs
touch types.rs state.rs constants.rs
```

##### ステップ1.2: 定数を `constants.rs` に移行
```rust
// crates/nanobot-core/src/service/http/constants.rs
use once_cell::sync::Lazy;
use regex::Regex;

/// DynamoDB sort key constant
pub const SK_PROFILE: &str = "PROFILE";

/// Response deadline in seconds
pub const RESPONSE_DEADLINE_SECS: u64 = 12;

/// GitHub tool names (admin only)
pub const GITHUB_TOOL_NAMES: &[&str] = &[
    "github_read_file",
    "github_create_or_update_file",
    "github_create_pr",
];

/// URL extraction regex
pub static URL_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"https?://[^\s<>"']+"#).unwrap()
});

/// Meta-cognition prompt (Japanese)
pub const META_INSTRUCTION_JA: &str = "...";

/// Meta-cognition prompt (English)
pub const META_INSTRUCTION_EN: &str = "...";

/// Base index.html content
pub static INDEX_HTML: &str = include_str!("../../../../../web/index.html");

/// Localized OGP pages
pub static LOCALIZED_PAGES: Lazy<std::collections::HashMap<&'static str, String>> = Lazy::new(|| {
    // ... (現在の実装をコピー)
});

/// Agent profiles
pub const AGENTS: &[super::types::AgentProfile] = &[
    // ... (現在の実装をコピー)
];

/// Promotion start date
pub const PROMO_START: &str = "2025-02-01T00:00:00+09:00";
```

##### ステップ1.3: `http/mod.rs` で再エクスポート
```rust
// crates/nanobot-core/src/service/http/mod.rs
pub mod constants;
pub use constants::*; // 既存のインポートを壊さないため
```

##### ステップ1.4: テスト
```bash
cargo test -p nanobot-core
cargo build --release --target aarch64-unknown-linux-gnu
```

**成果物**: 定数が `constants.rs` に移動、既存コード動作確認

---

#### Phase 2: 型定義の分離 (半日)

##### ステップ2.1: `types.rs` 作成
```rust
// crates/nanobot-core/src/service/http/types.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct UserProfile {
    pub user_id: String,
    pub display_name: Option<String>,
    pub plan: String,
    pub credits_remaining: i64,
    pub credits_used: i64,
    pub channels: Vec<String>,
    pub stripe_customer_id: Option<String>,
    pub email: Option<String>,
    pub created_at: String,
    pub last_passive_grant: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    // ... (32個の型をすべて移行)
}

// ... (全型定義をここに)
```

##### ステップ2.2: `http/mod.rs` で再エクスポート
```rust
// crates/nanobot-core/src/service/http/mod.rs
pub mod types;
pub use types::*; // 既存のインポートを壊さないため
```

##### ステップ2.3: テスト
```bash
cargo test -p nanobot-core
```

**成果物**: Request/Response型が `types.rs` に移動

---

#### Phase 3: AppState の分離 (半日)

##### ステップ3.1: `state.rs` 作成
```rust
// crates/nanobot-core/src/service/http/state.rs
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use tokio::sync::Mutex;

use crate::config::Config;
use crate::session::store::SessionStore;
use crate::provider::{self, LlmProvider};
use crate::service::integrations::ToolRegistry;

pub struct AppState {
    pub config: Config,
    pub sessions: Mutex<Box<dyn SessionStore>>,
    pub provider: Option<Arc<dyn LlmProvider>>,
    pub lb_provider: Option<Arc<dyn LlmProvider>>,
    pub lb_raw: Option<Arc<provider::LoadBalancedProvider>>,
    pub tool_registry: ToolRegistry,
    pub concurrent_requests: dashmap::DashMap<String, AtomicU32>,
    #[cfg(feature = "dynamodb-backend")]
    pub dynamo_client: Option<aws_sdk_dynamodb::Client>,
    #[cfg(feature = "dynamodb-backend")]
    pub config_table: Option<String>,
    pub ping_cache: Mutex<Option<(std::time::Instant, serde_json::Value)>>,
}

impl AppState {
    // ... (実装をコピー)
}
```

##### ステップ3.2: `http/mod.rs` で再エクスポート
```rust
pub mod state;
pub use state::AppState;
```

**成果物**: AppState が `state.rs` に移動

---

#### Phase 4: ユーティリティ関数の分離 (1日)

##### ステップ4.1: 認証関数 (`utils/auth.rs`)
```rust
// crates/nanobot-core/src/service/http/utils/auth.rs

/// Check if a session key, user ID, or email is an admin.
pub fn is_admin(key: &str) -> bool {
    let keys = std::env::var("ADMIN_SESSION_KEYS").unwrap_or_default();
    keys.split(',').map(|k| k.trim()).any(|k| !k.is_empty() && k == key)
}

#[cfg(feature = "dynamodb-backend")]
pub async fn resolve_session_key(
    dynamo: &aws_sdk_dynamodb::Client,
    table: &str,
    id: &str,
) -> String {
    // ... (実装をコピー)
}

#[cfg(feature = "dynamodb-backend")]
pub fn is_session_id(text: &str) -> bool {
    // ... (実装をコピー)
}

// ... (他の認証関連関数)
```

##### ステップ4.2: 課金関数 (`utils/billing.rs`)
```rust
// crates/nanobot-core/src/service/http/utils/billing.rs

#[cfg(feature = "dynamodb-backend")]
pub async fn get_or_create_user(
    dynamo: &aws_sdk_dynamodb::Client,
    table: &str,
    user_id: &str,
) -> super::super::types::UserProfile {
    // ... (実装をコピー)
}

#[cfg(feature = "dynamodb-backend")]
pub async fn deduct_credits(
    dynamo: &aws_sdk_dynamodb::Client,
    table: &str,
    user_id: &str,
    amount: i64,
) -> i64 {
    // ... (実装をコピー)
}

// ... (他の課金関連関数)
```

##### ステップ4.3: メモリ関数 (`utils/memory.rs`)
```rust
// crates/nanobot-core/src/service/http/utils/memory.rs

#[cfg(feature = "dynamodb-backend")]
pub async fn read_memory_context(
    dynamo: &aws_sdk_dynamodb::Client,
    table: &str,
    user_id: &str,
) -> String {
    // ... (実装をコピー)
}

#[cfg(feature = "dynamodb-backend")]
pub async fn append_daily_memory(
    dynamo: &aws_sdk_dynamodb::Client,
    table: &str,
    user_id: &str,
    text: &str,
) {
    // ... (実装をコピー)
}

// ... (他のメモリ関連関数)
```

##### ステップ4.4: 監査ログ (`utils/audit.rs`)
```rust
// crates/nanobot-core/src/service/http/utils/audit.rs

#[cfg(feature = "dynamodb-backend")]
pub fn emit_audit_log(
    dynamo: aws_sdk_dynamodb::Client,
    config_table: String,
    event_type: &str,
    user_id: &str,
    email: &str,
    details: &str,
) {
    // ... (実装をコピー)
}

#[cfg(feature = "dynamodb-backend")]
pub async fn check_rate_limit(
    dynamo: &aws_sdk_dynamodb::Client,
    table: &str,
    key: &str,
    limit: i64,
) -> bool {
    // ... (実装をコピー)
}
```

##### ステップ4.5: `utils/mod.rs` で再エクスポート
```rust
// crates/nanobot-core/src/service/http/utils/mod.rs
pub mod auth;
pub mod billing;
pub mod memory;
pub mod audit;

// 既存コード互換性のため、全関数を再エクスポート
pub use auth::*;
pub use billing::*;
pub use memory::*;
pub use audit::*;
```

##### ステップ4.6: `http/mod.rs` で再エクスポート
```rust
// crates/nanobot-core/src/service/http/mod.rs
pub mod utils;
pub use utils::*; // 既存のインポートを壊さないため
```

##### ステップ4.7: テスト
```bash
cargo test -p nanobot-core
cargo build --release --target aarch64-unknown-linux-gnu
```

**成果物**: 25個のユーティリティ関数が `utils/*` に移動、既存コード動作確認

---

#### Phase 5: `http/mod.rs` のスリム化 (半日)

##### ステップ5.1: `http/mod.rs` を最小化
```rust
// crates/nanobot-core/src/service/http/mod.rs
use std::sync::Arc;
use axum::{routing::{delete, get, post}, Router};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::compression::CompressionLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use axum::http;
use tracing::info;

// 子モジュール
pub mod constants;
pub mod types;
pub mod state;
pub mod utils;
mod handlers;

// 再エクスポート (既存コード互換性のため)
pub use constants::*;
pub use types::*;
pub use state::AppState;
pub use utils::*;
use handlers::*;

/// Get base URL for this instance.
pub fn get_base_url() -> String {
    std::env::var("BASE_URL").unwrap_or_else(|_| "https://chatweb.ai".to_string())
}

/// Create the axum Router with all API routes.
pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(handle_root))
        .route("/ja", get(handle_localized_root))
        // ... (143エンドポイント — 変更なし)
        .layer(RequestBodyLimitLayer::new(1024 * 1024))
        .layer(CompressionLayer::new())
        // ... (CORS, CSP設定 — 変更なし)
        .with_state(state)
}

/// Serve HTTP API on the given address.
pub async fn serve(addr: &str, state: Arc<AppState>) -> anyhow::Result<()> {
    let router = create_router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("HTTP server listening on {}", addr);
    axum::serve(listener, router).await?;
    Ok(())
}
```

**結果**: `http/mod.rs` が **200行** に削減 (元 3,663行 → **95%削減**)

##### ステップ5.2: 最終テスト
```bash
cargo test -p nanobot-core
cargo clippy -p nanobot-core
cargo build --release --target aarch64-unknown-linux-gnu
./infra/deploy-fast.sh
```

**成果物**: リファクタリング完了、本番デプロイ

---

#### Phase 6: ハンドラーのインポート最適化 (オプション)

**目的**: より明示的なインポートに変更 (段階的に実施)

##### Before (現在)
```rust
// handlers/chat.rs
use super::super::*; // 何がインポートされているか不明
```

##### After (最適化後)
```rust
// handlers/chat.rs
use crate::service::http::{
    state::AppState,
    types::{ChatRequest, ChatResponse, UserSettings},
    utils::auth::{is_admin, resolve_session_key, get_or_create_user},
    utils::billing::deduct_credits,
    utils::memory::{read_memory_context, append_daily_memory, spawn_consolidate_memory},
    constants::{SK_PROFILE, URL_REGEX, RESPONSE_DEADLINE_SECS, META_INSTRUCTION_JA, META_INSTRUCTION_EN},
};
```

**メリット**:
- インポート元が明確
- 未使用インポートの検出が容易
- IDE の補完が効きやすい

**実施方法**:
1. 1ファイルずつ最適化 (chat.rs → auth.rs → ...)
2. 各変更後にテスト実行
3. コミット単位: 1ファイル1コミット

**タイムライン**: 1週間 (1ファイル/日)

---

## 6. マイルストーン & チェックリスト

### Week 1: Phase 1-5 (基礎リファクタリング)

- [ ] **Day 1**: Phase 1 完了 (定数分離)
  - [ ] `constants.rs` 作成
  - [ ] 定数移行 (10個)
  - [ ] テスト通過確認
  - [ ] コミット: `refactor: extract constants to separate module`

- [ ] **Day 2**: Phase 2-3 完了 (型定義・AppState分離)
  - [ ] `types.rs` 作成 (32型)
  - [ ] `state.rs` 作成
  - [ ] テスト通過確認
  - [ ] コミット: `refactor: extract types and state to separate modules`

- [ ] **Day 3-4**: Phase 4 完了 (ユーティリティ分離)
  - [ ] `utils/auth.rs` 作成 (4関数)
  - [ ] `utils/billing.rs` 作成 (7関数)
  - [ ] `utils/memory.rs` 作成 (4関数)
  - [ ] `utils/audit.rs` 作成 (3関数)
  - [ ] テスト通過確認
  - [ ] コミット: `refactor: extract utility functions to utils module`

- [ ] **Day 5**: Phase 5 完了 (mod.rsスリム化)
  - [ ] `http/mod.rs` を200行に削減
  - [ ] 全テスト通過確認
  - [ ] Clippy警告ゼロ確認
  - [ ] ローカルHTTPサーバー起動確認
  - [ ] Lambda関数ビルド確認
  - [ ] コミット: `refactor: finalize http module restructuring`
  - [ ] **Staging デプロイ & 動作確認**

### Week 2: Phase 6 (オプション — インポート最適化)

- [ ] **Day 1**: `handlers/chat.rs` インポート最適化
- [ ] **Day 2**: `handlers/auth.rs` インポート最適化
- [ ] **Day 3**: `handlers/billing.rs` インポート最適化
- [ ] **Day 4**: `handlers/misc.rs` インポート最適化
- [ ] **Day 5**: 残りハンドラー最適化 + **本番デプロイ**

---

## 7. リスク管理

### リスク1: ビルドエラー

**発生確率**: 中
**影響度**: 高
**対策**:
- 各Phase後に必ずテスト実行
- `pub use` で既存インポートパスを維持
- Clippy でコンパイル警告をゼロに保つ

### リスク2: 動作不良 (ランタイムエラー)

**発生確率**: 低
**影響度**: 高
**対策**:
- Staging環境で動作確認
- 全APIエンドポイントの統合テスト実行
- ロールバック手順を事前確認

### リスク3: パフォーマンス劣化

**発生確率**: 極低
**影響度**: 中
**対策**:
- リファクタリングはロジック変更なし (移動のみ)
- ベンチマークで確認 (レイテンシ変化なし)

### リスク4: マージコンフリクト

**発生確率**: 中
**影響度**: 低
**対策**:
- 小さいコミット単位で進行
- Phase完了ごとに main にマージ
- 並行開発を一時停止 (1週間)

---

## 8. 期待される効果

### 定量的効果

| 指標 | Before | After | 改善率 |
|------|--------|-------|--------|
| `http/mod.rs` 行数 | 3,663行 | 200行 | **-95%** |
| 最大ファイルサイズ | 156KB | 20KB | **-87%** |
| 認知負荷 (関数数/ファイル) | 25関数 | 2関数 | **-92%** |
| モジュール数 | 1個 | 6個 | **+500%** |

### 定性的効果

#### 可読性
- ✅ **責任が明確** — 認証は `auth.rs`, 課金は `billing.rs` に集約
- ✅ **インポート元が明確** — `use crate::service::http::utils::auth::is_admin`
- ✅ **新規メンバーのオンボーディング容易** — 小さいモジュールで理解しやすい

#### 保守性
- ✅ **変更影響範囲が明確** — `auth.rs` 変更時は認証機能のみ影響
- ✅ **テストが容易** — モジュール単位でテスト可能
- ✅ **並行開発が可能** — 複数人で異なるモジュールを同時に変更可能

#### 拡張性
- ✅ **新機能追加が容易** — 新しいutilsモジュール追加で拡張
- ✅ **ドメイン層への移行準備** — 将来DDDアーキテクチャへの移行が容易

---

## 9. 結論

### サマリー

1. **`http/mod.rs` は削除不可** — Lambda関数、ローカル開発、全ハンドラーが依存
2. **現状の問題** — 3,663行の巨大モジュール、責任が混在、保守性が低い
3. **推奨アプローチ** — **オプション3** (抽象化レイヤー分割)
4. **実装期間** — **1週間** (Phase 1-5) + オプション1週間 (Phase 6)
5. **期待効果** — コード量 **95%削減**、保守性大幅向上、並行開発可能に

### 次のアクション

#### 即座に実施
```bash
git checkout -b refactor/http-module-split
cd crates/nanobot-core/src/service/http/
mkdir utils
touch constants.rs types.rs state.rs utils/{mod,auth,billing,memory,audit}.rs
```

#### 1週間後の目標
- [ ] `http/mod.rs` を 200行に削減
- [ ] 全テスト通過
- [ ] Staging デプロイ成功
- [ ] 本番デプロイ準備完了

---

## 10. 参考資料

### 関連ドキュメント
- `/Users/yuki/workspace/ai/nanobot/CLAUDE.md` — プロジェクト概要
- `/Users/yuki/workspace/ai/nanobot/.claude/rules/rust-backend.md` — Rustバックエンド規約
- `crates/nanobot-core/src/service/http/mod.rs` — 現在の実装

### 外部リンク
- [The Rust Programming Language — Modules](https://doc.rust-lang.org/book/ch07-02-defining-modules-to-control-scope-and-privacy.html)
- [Rust API Guidelines — Module organization](https://rust-lang.github.io/api-guidelines/naming.html)
- [Clean Code — Single Responsibility Principle](https://en.wikipedia.org/wiki/Single-responsibility_principle)

---

**作成者**: Claude Sonnet 4.5
**最終更新**: 2026-02-15
