# nanobot デプロイ手順書

## 前提条件
- Rust (stable) + cargo-zigbuild + zig
- AWS CLI (configured, ap-northeast-1)
- SAM CLI

## クイックデプロイ（推奨）

### Step 1: クロスコンパイル
```bash
RUSTUP_TOOLCHAIN=stable \
RUSTC=/Users/yuki/.rustup/toolchains/stable-aarch64-apple-darwin/bin/rustc \
cargo zigbuild \
  --manifest-path crates/nanobot-lambda/Cargo.toml \
  --release \
  --target aarch64-unknown-linux-gnu
```

### Step 2: ZIP作成
```bash
cp target/aarch64-unknown-linux-gnu/release/bootstrap target/lambda/bootstrap/bootstrap
chmod +x target/lambda/bootstrap/bootstrap
cd target/lambda/bootstrap && zip -j /tmp/nanobot-lambda.zip bootstrap && cd -
```

### Step 3: Lambda更新
```bash
aws lambda update-function-code \
  --function-name nanobot \
  --zip-file fileb:///tmp/nanobot-lambda.zip \
  --region ap-northeast-1
```

### Step 4: バージョン発行 + liveエイリアス更新
```bash
# 新バージョンを発行
VERSION=$(aws lambda publish-version \
  --function-name nanobot \
  --region ap-northeast-1 \
  --query 'Version' --output text)

echo "Published version: $VERSION"

# liveエイリアスを更新（本番反映）
aws lambda update-alias \
  --function-name nanobot \
  --name live \
  --function-version "$VERSION" \
  --region ap-northeast-1
```

### Step 5: 確認
```bash
# エイリアス確認
aws lambda get-alias \
  --function-name nanobot \
  --name live \
  --region ap-northeast-1

# UI確認
curl -s https://chatweb.ai/ | head -50

# API確認
curl -s -o /dev/null -w "%{http_code}" https://chatweb.ai/api/v1/auth/me
```

## SAMデプロイ（インフラ変更時）

template.yaml の変更（環境変数追加、IAMポリシー変更等）がある場合はSAMを使う：

```bash
cd infra && bash deploy.sh
```

**注意**: SAMデプロイ後も `live` エイリアスの更新が必要。
SAMは `$LATEST` を更新するが、API Gatewayは `live` エイリアスを参照する。

## 環境変数の更新（コード変更なし）

```bash
aws lambda update-function-configuration \
  --function-name nanobot \
  --region ap-northeast-1 \
  --environment file:///tmp/env_vars.json
```

env_vars.json のフォーマット:
```json
{
  "Variables": {
    "KEY": "value",
    ...
  }
}
```

## 重要な注意点

1. **HTMLはバイナリに埋め込み**: `include_str!()` を使用。HTML変更後は必ずリビルド（Step 1から）
2. **`live`エイリアス必須**: `update-function-code` だけでは本番に反映されない。必ず `publish-version` → `update-alias` を実行
3. **カナリアデプロイ**: CI/CDでは canary 10% → production 100% のフロー。手動デプロイ時は即時100%反映
4. **ロールバック**: 前のバージョンに戻すには `update-alias --function-version <前のバージョン番号>` を実行

## 現在の状態
- Lambda: v86 (`live` alias)
- リージョン: ap-northeast-1
- ランタイム: provided.al2023 (ARM64)
- API Gateway: chatweb.ai / api.chatweb.ai
