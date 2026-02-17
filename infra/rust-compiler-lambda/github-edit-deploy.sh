#!/bin/bash
set -euo pipefail

# GitHub → AI編集 → コンパイル → Lambda デプロイ
# 使い方: ./github-edit-deploy.sh <github-url> "編集指示" <function-name>
#
# 例:
# ./github-edit-deploy.sh https://github.com/example/rust-app "main関数に計算機能を追加" my-calc-function

GITHUB_URL="${1:-}"
EDIT_INSTRUCTION="${2:-}"
FUNCTION_NAME="${3:-auto-deployed-function}"
REGION="${AWS_REGION:-ap-northeast-1}"

if [ -z "$GITHUB_URL" ]; then
    echo "使い方: $0 <github-url> \"編集指示\" [function-name]"
    echo ""
    echo "例:"
    echo "  $0 https://github.com/rust-lang/rust-by-example \"Hello Worldに変更\" hello-function"
    exit 1
fi

WORK_DIR="/tmp/github-deploy-$$"
mkdir -p "$WORK_DIR"
cd "$WORK_DIR"

echo "🎯 GitHub → AI編集 → コンパイル → Lambda デプロイパイプライン"
echo "================================================================"
echo ""
echo "📦 Step 1: GitHubからクローン"
echo "URL: $GITHUB_URL"

# GitHubリポジトリをクローン
git clone --depth 1 "$GITHUB_URL" repo
cd repo

# Rustプロジェクトを探す
if [ ! -f "Cargo.toml" ]; then
    echo "❌ Cargo.tomlが見つかりません。Rustプロジェクトではないかもしれません。"
    exit 1
fi

PROJECT_NAME=$(grep -m 1 'name = ' Cargo.toml | cut -d'"' -f2)
echo "✅ プロジェクト: $PROJECT_NAME"
echo ""

if [ -n "$EDIT_INSTRUCTION" ]; then
    echo "🤖 Step 2: nanobotに編集を依頼"
    echo "指示: $EDIT_INSTRUCTION"

    # nanobotに編集を依頼
    cat > /tmp/edit-request.txt << EOF
このRustプロジェクトを編集してください。

プロジェクト構造:
$(find . -name "*.rs" -type f | head -20)

編集指示: $EDIT_INSTRUCTION

変更が必要なファイルの内容を教えてください。
ファイルパスと新しい内容を明確に示してください。
EOF

    echo "nanobotに問い合わせ中..."
    EDIT_RESPONSE=$(nanobot agent -m "$(cat /tmp/edit-request.txt)")

    echo "$EDIT_RESPONSE"
    echo ""
    echo "⚠️  AIの提案を確認して、手動で適用するか、自動適用を選択してください"
    read -p "自動適用しますか？ (y/n): " AUTO_APPLY

    if [ "$AUTO_APPLY" = "y" ]; then
        echo "TODO: AI応答からファイル変更を抽出して適用"
        # ここでAIの応答をパースして実際にファイルを編集
    fi
else
    echo "⏭️  Step 2: スキップ (編集指示なし)"
fi

echo ""
echo "🔨 Step 3: Rustコンパイル"

# Lambda用にコンパイル（ARM64）
if command -v cross &> /dev/null; then
    echo "Using cross for ARM64 build..."
    cross build --release --target aarch64-unknown-linux-musl
    BINARY="target/aarch64-unknown-linux-musl/release/$PROJECT_NAME"
else
    echo "Using cargo (native build)..."
    cargo build --release
    BINARY="target/release/$PROJECT_NAME"
fi

if [ ! -f "$BINARY" ]; then
    echo "❌ コンパイル失敗"
    exit 1
fi

echo "✅ コンパイル成功: $BINARY"
echo ""

echo "📦 Step 4: Lambda用にパッケージング"

# Lambdaブートストラップとして配置
mkdir -p package
cp "$BINARY" package/bootstrap
chmod +x package/bootstrap

# ZIP化
cd package
zip -q deployment.zip bootstrap
DEPLOYMENT_PACKAGE="$WORK_DIR/repo/package/deployment.zip"

echo "✅ パッケージ作成: $(du -h deployment.zip | cut -f1)"
echo ""

echo "🚀 Step 5: Lambda にデプロイ"

# IAMロールの確認/作成
ROLE_NAME="lambda-rust-execution-role"
ROLE_ARN=$(aws iam get-role --role-name "$ROLE_NAME" --query 'Role.Arn' --output text 2>/dev/null || echo "")

if [ -z "$ROLE_ARN" ]; then
    echo "IAMロールを作成中..."

    cat > /tmp/trust-policy.json << 'TRUST'
{
  "Version": "2012-10-17",
  "Statement": [{
    "Effect": "Allow",
    "Principal": {"Service": "lambda.amazonaws.com"},
    "Action": "sts:AssumeRole"
  }]
}
TRUST

    ROLE_ARN=$(aws iam create-role \
        --role-name "$ROLE_NAME" \
        --assume-role-policy-document file:///tmp/trust-policy.json \
        --query 'Role.Arn' \
        --output text)

    aws iam attach-role-policy \
        --role-name "$ROLE_NAME" \
        --policy-arn "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole"

    echo "⏳ IAMロールの伝播を待機中..."
    sleep 10
fi

# Lambda関数の作成/更新
if aws lambda get-function --function-name "$FUNCTION_NAME" --region "$REGION" 2>/dev/null; then
    echo "既存の関数を更新中..."
    aws lambda update-function-code \
        --function-name "$FUNCTION_NAME" \
        --zip-file "fileb://$DEPLOYMENT_PACKAGE" \
        --region "$REGION" \
        --query 'FunctionArn' \
        --output text
else
    echo "新規関数を作成中..."
    aws lambda create-function \
        --function-name "$FUNCTION_NAME" \
        --runtime provided.al2023 \
        --role "$ROLE_ARN" \
        --handler bootstrap \
        --zip-file "fileb://$DEPLOYMENT_PACKAGE" \
        --architectures arm64 \
        --timeout 30 \
        --memory-size 512 \
        --region "$REGION" \
        --query 'FunctionArn' \
        --output text
fi

echo ""
echo "================================"
echo "✅ デプロイ完了！"
echo "================================"
echo ""
echo "📊 詳細:"
echo "  関数名: $FUNCTION_NAME"
echo "  リージョン: $REGION"
echo "  パッケージ: $(du -h $DEPLOYMENT_PACKAGE | cut -f1)"
echo ""
echo "🧪 テスト実行:"
echo "  aws lambda invoke --function-name $FUNCTION_NAME --region $REGION response.json"
echo "  cat response.json"
echo ""

# クリーンアップ
cd /
rm -rf "$WORK_DIR"
