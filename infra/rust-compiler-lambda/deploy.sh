#!/bin/bash
set -e

REGION="ap-northeast-1"
FUNCTION_NAME="rust-compiler-lambda"
ECR_REPO="rust-compiler-lambda"
AWS_ACCOUNT_ID=$(aws sts get-caller-identity --query Account --output text)

echo "🦀 Building Rust Compiler Lambda"
echo "================================"

# ECRリポジトリ作成
aws ecr create-repository --repository-name $ECR_REPO --region $REGION 2>/dev/null || true

# Dockerイメージをビルド
docker build -t $ECR_REPO:latest .

# ECRにプッシュ
aws ecr get-login-password --region $REGION | docker login --username AWS --password-stdin $AWS_ACCOUNT_ID.dkr.ecr.$REGION.amazonaws.com
docker tag $ECR_REPO:latest $AWS_ACCOUNT_ID.dkr.ecr.$REGION.amazonaws.com/$ECR_REPO:latest
docker push $AWS_ACCOUNT_ID.dkr.ecr.$REGION.amazonaws.com/$ECR_REPO:latest

# Lambda関数を作成/更新
if aws lambda get-function --function-name $FUNCTION_NAME --region $REGION 2>/dev/null; then
  echo "Updating existing function..."
  aws lambda update-function-code \
    --function-name $FUNCTION_NAME \
    --image-uri $AWS_ACCOUNT_ID.dkr.ecr.$REGION.amazonaws.com/$ECR_REPO:latest \
    --region $REGION
else
  echo "Creating new function..."
  aws lambda create-function \
    --function-name $FUNCTION_NAME \
    --package-type Image \
    --code ImageUri=$AWS_ACCOUNT_ID.dkr.ecr.$REGION.amazonaws.com/$ECR_REPO:latest \
    --role arn:aws:iam::$AWS_ACCOUNT_ID:role/lambda-execution-role \
    --timeout 300 \
    --memory-size 2048 \
    --region $REGION
fi

echo ""
echo "✅ Deployed!"
echo "Test: aws lambda invoke --function-name $FUNCTION_NAME --payload '{\"code\":\"fn main() { println!(\\\"Hello from Lambda!\\\"); }\"}' response.json"
