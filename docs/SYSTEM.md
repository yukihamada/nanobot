# nanobot システム全体構成

> バージョン: 2.0.0
> 最終更新: 2026-02-18

## 目次
1. [コアシステム](#1-コアシステム)
2. [拡張システム](#2-拡張システム)
3. [インフラストラクチャ](#3-インフラストラクチャ)
4. [テスト体系](#4-テスト体系)
5. [改善提案](#5-改善提案)

---

## 1. コアシステム

### 1.1 基本機能

#### CLIインタラクション
- コマンドライン対話インターフェース
- ストリーミングレスポンス対応
- シェルコマンド実行と結果表示
- プログレスバー表示機能
- 履歴管理とコンテキスト保持

#### Voice UI
- 音声入力認識（STT - Web Speech API）
- テキスト音声変換（TTS - OpenAI tts-1, nova）
- 複数言語対応（日本語・英語）
- Push-to-talk UI
- Auto-TTS（音声入力時に応答を自動音声再生）

#### ファイル操作
- 読み取り（read_file）
  - テキストファイル
  - バイナリファイル
  - 大容量ファイル対応
- 書き込み（write_file）
  - 新規作成
  - 追記モード
  - 権限管理
- 編集（edit_file）
  - インプレース編集
  - バックアップ作成
  - 差分管理
- ディレクトリ操作（list_dir）
  - 再帰的リスト
  - フィルタリング
  - ソート機能

#### シェルコマンド実行
- サンドボックス実行（`/tmp/sandbox/{session_id}/`）
- 言語サポート: shell/Python/Node.js
- セキュリティガードパターン
- タイムアウト制御（10秒）

#### Web検索 & フェッチ
- マルチプロバイダー検索（Brave/Bing/Jina）
- コンテンツ抽出（Jina Reader）
- レート制限
- キャッシュ管理

#### マルチチャネルメッセージング
- リアルタイムSSEストリーミング
- チャネル同期（/link コマンド）
- QRコード連携
- Deep link対応

#### バックグラウンドタスク管理
- 非同期タスク実行
- プログレストラッキング
- タスクキャンセル機能

#### Agentic Mode
- マルチイテレーションツールループ
- プラン別制限（Free=1, Starter=3, Pro=5）
- ツール並列実行（最大5並列）
- SSE進捗イベント（tool_start/tool_result/thinking）

### 1.2 サポートチャネル

| チャネル | プラットフォーム | 特性 |
|---------|--------------|------|
| CLI | ターミナル | 開発者向け、全機能利用可 |
| Voice | Web Speech API | 音声中心、Push-to-talk |
| Web | SPA (index.html) | 最賢モデル、全機能 |
| LINE | @619jcqqh | 日本市場向け、200字制限 |
| Telegram | @chatweb_ai_bot | グローバル、Markdown対応 |
| Discord | Webhook | コミュニティ向け |
| WhatsApp | Business API | グローバル市場 |
| Teams | Webhook | 企業向け |
| Slack | Webhook | 企業向け |
| Facebook | Messenger | ソーシャル連携 |

### 1.3 コア特性

- **名前**: nanobot
- **バージョン**: 2.0.0
- **パーソナリティ**: 好奇心旺盛、積極的、技術的正確性
- **ランタイム**: Rust on AWS Lambda (ARM64)
- **メモリシステム**: DynamoDB + ベクトル検索（semantic memory）
- **モデル選択**: 自動フェイルオーバー（Anthropic → OpenAI → Google）
- **クレジットシステム**: 天井除算、最低1クレジット/コール

---

## 2. 拡張システム

### 2.1 nanobot-core

**パス**: `crates/nanobot-core/`

#### 主要モジュール

| モジュール | 役割 | 主要ファイル |
|-----------|------|------------|
| **agent** | エージェント実行エンジン | `agent/mod.rs`, `agent/personality.rs` |
| **provider** | LLMプロバイダー統合 | `provider/mod.rs`, `provider/openai_compat.rs`, `provider/embeddings.rs` |
| **memory** | 長期記憶システム | `memory/dynamo_backend.rs`, `memory/backend.rs` |
| **service** | HTTPサービス | `service/http.rs`, `service/commands.rs`, `service/integrations.rs` |
| **channel** | チャネル統合 | `channel/line.rs`, `channel/telegram.rs`, `channel/facebook.rs` |
| **tools** | ツールレジストリ | `tools/registry.rs`, `tools/builtin.rs` |

#### 機能
- ツールシステム（35 built-in tools: 24 core + 11 optional）
- メモリ管理（デイリーログ + 長期記憶 + ベクトル検索）
- チャネル統合（13チャネル対応）
- プロバイダーフェイルオーバー（LoadBalancedProvider）
- SSEストリーミング
- クレジット計算・差引

### 2.2 nanobot-lambda

**パス**: `crates/nanobot-lambda/`

#### 機能
- AWS Lambda対応（`provided.al2023` runtime）
- ARM64最適化（Graviton3）
- コールドスタート <50ms
- API Gateway v2統合
- カスタムドメイン対応（chatweb.ai, api.chatweb.ai）

#### ビルド
```bash
RUSTUP_TOOLCHAIN=stable \
RUSTC=/Users/yuki/.rustup/toolchains/stable-aarch64-apple-darwin/bin/rustc \
cargo zigbuild --manifest-path crates/nanobot-lambda/Cargo.toml \
  --release --target aarch64-unknown-linux-gnu
```

---

## 3. インフラストラクチャ

### 3.1 AWS構成

| サービス | リソース | 用途 |
|---------|---------|------|
| Lambda | nanobot function | メインアプリケーション |
| API Gateway | 3nitmtxgsi | HTTPエンドポイント |
| DynamoDB | nanobot-messages | セッション・ユーザー・メモリ |
| CloudWatch | ログ・メトリクス | モニタリング |

**リージョン**: ap-northeast-1（東京）

### 3.2 CI/CD

**パイプライン**: GitHub Actions

```
main branch push
  ↓
test (cargo test)
  ↓
build (cargo zigbuild)
  ↓
canary (10%)
  ↓
production (100%, manual approval)
```

**ワークフロー**: `.github/workflows/deploy.yml`

### 3.3 モニタリング

- **CloudWatch Logs**: Lambda実行ログ
- **CloudWatch Metrics**: カスタムメトリクス（クレジット消費、API呼び出し数）
- **DynamoDB監査ログ**: `AUDIT#{date}` レコード（90日TTL）
- **エラートラッキング**: Rust `tracing` クレート

---

## 4. テスト体系

### 4.1 ユニットテスト

**パス**: `tests/`

| テストカテゴリ | ファイル | 対象 |
|-------------|---------|------|
| ツールレジストリ | `test_tool_registry_builtins` | ツール数検証（15 base, 18 with GitHub） |
| プロバイダー | `provider/tests.rs` | APIレスポンス解析 |
| メモリ | `memory/tests.rs` | DynamoDB操作 |
| コマンド | `service/commands.rs` | スラッシュコマンド解析 |

### 4.2 統合テスト

**実行**: `cargo test --all`

- チャネル統合テスト（LINE/Telegram Webhook）
- SSEストリーミングテスト
- クレジット計算テスト
- 認証フローテスト

### 4.3 パフォーマンステスト

- コールドスタート計測（<50ms目標）
- スループット測定（同時接続数）
- メモリ使用量プロファイリング

---

## 5. 改善提案

### 5.1 スキルシステムの強化

#### プラグイン機構
- **目的**: サードパーティツール統合を簡素化
- **実装案**:
  - WASM/DLL動的ロード
  - Trait-based plugin API
  - バージョン管理（Semantic Versioning）
- **セキュリティ**: サンドボックス実行、権限管理

#### スキルマーケットプレイス
- **目的**: コミュニティ貢献を促進
- **機能**:
  - スキル検索・インストール
  - レーティング・レビューシステム
  - 自動更新
- **収益化**: Developer Program（有料スキル販売）

### 5.2 メモリシステムの最適化

#### キャッシュ層
- **現状**: DynamoDB直接アクセス
- **改善案**:
  - Redis/Elasticacheキャッシュ層追加
  - セッションキャッシュ（1時間TTL）
  - LRU eviction policy

#### ベクトル検索の高速化
- **現状**: クライアント側コサイン類似度計算
- **改善案**:
  - DynamoDB Streamsトリガーで埋め込み自動生成
  - Pinecone/Qdrant統合検討
  - 階層的インデックス（HNSW）

#### コンテキスト管理
- **目的**: トークン使用量削減
- **実装案**:
  - 会話要約（定期的にLLMで要約）
  - 重要度スコアリング（重要な会話を優先保持）
  - 自動アーカイブ（90日以上古い会話）

### 5.3 セキュリティ強化

#### 監査システム
- **機能**:
  - 全API呼び出しのロギング
  - 異常検知（突然のクレジット大量消費）
  - コンプライアンスレポート（GDPR/CCPA）

#### 権限管理
- **現状**: バイナリ（ユーザー vs 管理者）
- **改善案**:
  - ロールベースアクセス制御（RBAC）
  - 細粒度権限（ツール単位の許可/拒否）
  - OAuth2/OpenID Connect統合

#### 暗号化
- **機能**:
  - メッセージの暗号化（at-rest, in-transit）
  - エンドツーエンド暗号化（E2EE）オプション
  - 鍵管理（AWS KMS統合）

### 5.4 モニタリング拡張

#### 詳細メトリクス
- **追加指標**:
  - ツール実行時間（P50/P90/P99）
  - プロバイダー別成功率
  - チャネル別アクティブユーザー数
  - クレジット消費傾向

#### アラート設定
- **トリガー**:
  - エラー率 > 5%
  - レイテンシ P99 > 500ms
  - クレジット残高 < 100
- **通知先**: Slack, PagerDuty, Email

#### パフォーマンス分析
- **ツール**: AWS X-Ray, Honeycomb
- **分析対象**:
  - ボトルネック特定
  - 依存関係マップ
  - リクエストトレーシング

---

## 参考リンク

- [README.md](../README.md) - プロジェクト概要
- [SKILLS.md](SKILLS.md) - スキル詳細（英語版）
- [deployment.md](deployment.md) - デプロイガイド
- [environment-variables.md](environment-variables.md) - 環境変数一覧
- [CLAUDE.md](../CLAUDE.md) - 開発者向けガイド

---

**このドキュメントは nanobot v2.0.0 の全体像を示します。詳細な実装は各モジュールのドキュメントを参照してください。**
