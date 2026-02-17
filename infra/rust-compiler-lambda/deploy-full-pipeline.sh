#!/bin/bash
set -e

# フルパイプラインLambda関数をデプロイ
# この関数自体が、GitHub → 編集 → コンパイル → デプロイを実行する

REGION="ap-northeast-1"
FUNCTION_NAME="rust-full-pipeline"

echo "🚀 デプロイ: GitHub → 編集 → コンパイル → デプロイ Pipeline"
echo "=========================================================="

# Dockerfileを作成
cat > Dockerfile.pipeline << 'DOCKERFILE'
FROM public.ecr.aws/lambda/python:3.12

# 必要なツールをインストール
RUN yum install -y git gcc zip && \
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable && \
    . $HOME/.cargo/env && \
    rustup target add aarch64-unknown-linux-musl && \
    yum install -y gcc-aarch64-linux-gnu

ENV PATH="/root/.cargo/bin:${PATH}"

# Python依存関係
RUN pip install boto3

# Lambda関数コード
COPY full-pipeline-lambda.py ${LAMBDA_TASK_ROOT}

CMD ["full-pipeline-lambda.lambda_handler"]
DOCKERFILE

echo "📦 Dockerイメージをビルド中..."
docker build -f Dockerfile.pipeline -t $FUNCTION_NAME:latest .

# ECRにプッシュ
AWS_ACCOUNT_ID=$(aws sts get-caller-identity --query Account --output text)
ECR_URI="$AWS_ACCOUNT_ID.dkr.ecr.$REGION.amazonaws.com/$FUNCTION_NAME"

# ECRリポジトリ作成
aws ecr create-repository --repository-name $FUNCTION_NAME --region $REGION 2>/dev/null || true

# ECRにログイン
aws ecr get-login-password --region $REGION | \
    docker login --username AWS --password-stdin $ECR_URI

# タグ付け＆プッシュ
docker tag $FUNCTION_NAME:latest $ECR_URI:latest
docker push $ECR_URI:latest

# Lambda関数を作成/更新
echo "🚀 Lambda関数をデプロイ中..."

# IAMロール作成
ROLE_NAME="rust-pipeline-lambda-role"
ROLE_ARN=$(aws iam get-role --role-name $ROLE_NAME --query 'Role.Arn' --output text 2>/dev/null || echo "")

if [ -z "$ROLE_ARN" ]; then
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
        --role-name $ROLE_NAME \
        --assume-role-policy-document file:///tmp/trust-policy.json \
        --query 'Role.Arn' \
        --output text)

    # 必要なポリシーをアタッチ
    aws iam attach-role-policy \
        --role-name $ROLE_NAME \
        --policy-arn "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole"

    # Lambda作成・更新権限を追加
    cat > /tmp/lambda-policy.json << 'POLICY'
{
  "Version": "2012-10-17",
  "Statement": [{
    "Effect": "Allow",
    "Action": [
      "lambda:CreateFunction",
      "lambda:UpdateFunctionCode",
      "lambda:GetFunction",
      "iam:GetRole",
      "iam:CreateRole",
      "iam:AttachRolePolicy"
    ],
    "Resource": "*"
  }]
}
POLICY

    aws iam put-role-policy \
        --role-name $ROLE_NAME \
        --policy-name LambdaManagement \
        --policy-document file:///tmp/lambda-policy.json

    sleep 10
fi

# Lambda関数作成/更新
if aws lambda get-function --function-name $FUNCTION_NAME --region $REGION 2>/dev/null; then
    aws lambda update-function-code \
        --function-name $FUNCTION_NAME \
        --image-uri $ECR_URI:latest \
        --region $REGION
else
    aws lambda create-function \
        --function-name $FUNCTION_NAME \
        --package-type Image \
        --code ImageUri=$ECR_URI:latest \
        --role $ROLE_ARN \
        --timeout 900 \
        --memory-size 3008 \
        --region $REGION
fi

echo ""
echo "================================"
echo "✅ デプロイ完了！"
echo "================================"
echo ""
echo "🎯 使い方:"
echo ""
echo "aws lambda invoke \\"
echo "  --function-name $FUNCTION_NAME \\"
echo "  --payload '{"
echo "    \"github_url\": \"https://github.com/rust-lang/rust-by-example\","
echo "    \"edit_instruction\": \"Hello Worldに変更\","
echo "    \"function_name\": \"my-rust-app\","
echo "    \"use_ai_edit\": false"
echo "  }' \\"
echo "  --region $REGION \\"
echo "  response.json"
echo ""
echo "cat response.json"
