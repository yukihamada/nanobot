# 🚀 GitHub → AI編集 → コンパイル → Lambda デプロイ

GitHubからRustプロジェクトを取得し、AIで編集し、コンパイルして、Lambda上に自動デプロイ！

## 🎯 できること

1. **GitHubから取得**: 任意のRustリポジトリをクローン
2. **AI自動編集**: nanobotが自動でコード修正
3. **Lambda上でコンパイル**: ARM64向けにビルド
4. **自動デプロイ**: 新しいLambda関数として公開

## 🌟 2つのアプローチ

### 方法1: ローカルスクリプト（推奨）

```bash
cd /Users/yuki/workspace/ai/nanobot/infra/rust-compiler-lambda

# GitHubから取得→編集→デプロイ
./github-edit-deploy.sh \
  https://github.com/rust-lang/rust-by-example \
  "Hello Worldプログラムに変更" \
  hello-world-function
```

### 方法2: Lambda上で全実行（完全自動化）

```bash
# フルパイプラインLambda関数をデプロイ
./deploy-full-pipeline.sh

# 使用
aws lambda invoke \
  --function-name rust-full-pipeline \
  --payload '{
    "github_url": "https://github.com/rust-lang/rust-by-example",
    "edit_instruction": "計算機能を追加",
    "function_name": "my-calculator",
    "use_ai_edit": true
  }' \
  --region ap-northeast-1 \
  response.json
```

## 📊 パイプライン詳細

```
┌─────────────┐
│   GitHub    │  Rustプロジェクト
│ Repository  │
└──────┬──────┘
       │ git clone
       ↓
┌─────────────┐
│  AI Editor  │  nanobot/Claudeがコード編集
│  (nanobot)  │
└──────┬──────┘
       │ 編集後のコード
       ↓
┌─────────────┐
│  Compiler   │  cargo build (ARM64)
│   (Rust)    │
└──────┬──────┘
       │ バイナリ
       ↓
┌─────────────┐
│   Package   │  bootstrap として ZIP化
│             │
└──────┬──────┘
       │ deployment.zip
       ↓
┌─────────────┐
│AWS Lambda   │  新しい関数としてデプロイ
│   Function  │
└─────────────┘
```

## 🎮 使用例

### 例1: シンプルなHello World

```bash
./github-edit-deploy.sh \
  https://github.com/rust-lang/rust-by-example \
  "main関数をHello Worldだけにする" \
  hello-world
```

### 例2: 既存プロジェクトに機能追加

```bash
./github-edit-deploy.sh \
  https://github.com/user/calculator \
  "平方根計算機能を追加してください" \
  calculator-v2
```

### 例3: 完全自動（AI任せ）

```bash
# 編集指示なし = そのままコンパイル＆デプロイ
./github-edit-deploy.sh \
  https://github.com/user/rust-app \
  "" \
  deployed-app
```

## 🔧 技術詳細

### コンパイル

- **ターゲット**: `aarch64-unknown-linux-musl` (Lambda ARM64)
- **最適化**: `--release` ビルド
- **バイナリ名**: `bootstrap` (Lambda required.al2023)

### AI編集

```bash
# nanobotへの指示例
nanobot agent -m "
このRustプロジェクトを編集:
- src/main.rs の main関数を変更
- 計算機能を追加
- エラーハンドリングを改善
"
```

### デプロイ設定

| 項目 | 値 |
|------|-----|
| **Runtime** | provided.al2023 |
| **Architecture** | ARM64 |
| **Memory** | 512MB (調整可能) |
| **Timeout** | 30秒 (調整可能) |

## 🎯 実用シナリオ

### シナリオ1: OSSのフォーク＆カスタマイズ

```bash
# Rust製のCLIツールをLambda関数化
./github-edit-deploy.sh \
  https://github.com/sharkdp/bat \
  "CLI引数をLambdaイベントから受け取るように変更" \
  bat-lambda
```

### シナリオ2: 自動バージョンデプロイ

```bash
# 毎日最新版を自動デプロイ
for version in v1.0 v1.1 v1.2; do
  ./github-edit-deploy.sh \
    https://github.com/user/app/tree/$version \
    "" \
    "app-$version"
done
```

### シナリオ3: A/Bテスト

```bash
# 2つのバージョンを並行デプロイ
./github-edit-deploy.sh \
  https://github.com/user/app \
  "アルゴリズムAを使用" \
  app-algorithm-a

./github-edit-deploy.sh \
  https://github.com/user/app \
  "アルゴリズムBを使用" \
  app-algorithm-b
```

## ⚠️ 制約

1. **コンパイル時間**: 初回は1-3分（依存関係次第）
2. **Lambda制限**:
   - メモリ: 最大10GB
   - タイムアウト: 最大15分
   - /tmpストレージ: 最大10GB
3. **GitHubアクセス**: publicリポジトリのみ（privateは認証が必要）

## 🚀 高度な使い方

### カスタムビルドフラグ

```bash
# github-edit-deploy.sh 内で
export RUSTFLAGS="-C target-cpu=native -C opt-level=3"
cargo build --release
```

### 複数バイナリのデプロイ

```bash
# ワークスペース内の全バイナリをデプロイ
for binary in $(cargo metadata --format-version 1 | jq -r '.packages[].targets[] | select(.kind[] == "bin") | .name'); do
  ./github-edit-deploy.sh \
    https://github.com/user/workspace \
    "" \
    "workspace-$binary"
done
```

### CI/CD統合

```yaml
# .github/workflows/deploy-lambda.yml
name: Deploy to Lambda
on:
  push:
    branches: [main]

jobs:
  deploy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - name: Deploy to Lambda
        run: |
          ./infra/rust-compiler-lambda/github-edit-deploy.sh \
            ${{ github.repository }} \
            "" \
            production-function
        env:
          AWS_ACCESS_KEY_ID: ${{ secrets.AWS_ACCESS_KEY_ID }}
          AWS_SECRET_ACCESS_KEY: ${{ secrets.AWS_SECRET_ACCESS_KEY }}
```

## 📈 パフォーマンス

| プロジェクトサイズ | Clone | コンパイル | デプロイ | 合計 |
|------------------|-------|----------|---------|------|
| Small (Hello World) | 5秒 | 30秒 | 10秒 | ~45秒 |
| Medium (1000 LOC) | 10秒 | 60秒 | 15秒 | ~85秒 |
| Large (10k LOC) | 20秒 | 180秒 | 30秒 | ~230秒 |

## 🔗 次のステップ

1. **テストの自動実行**: デプロイ後に自動テスト
2. **ロールバック機能**: 前バージョンに戻す
3. **マルチリージョンデプロイ**: 複数リージョンに同時デプロイ
4. **メトリクス収集**: CloudWatchで性能監視

## 📚 関連ドキュメント

- [Lambda Rust Runtime](https://github.com/awslabs/aws-lambda-rust-runtime)
- [Cross-compilation Guide](https://rust-lang.github.io/rustup/cross-compilation.html)
- [nanobot Documentation](https://github.com/yukihamada/nanobot)
