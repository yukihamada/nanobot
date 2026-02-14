#!/bin/bash
# deploy-dev.sh - Deploy to dev environment (dev.chatweb.ai)
# Usage: ./scripts/deploy-dev.sh

set -e

FUNCTION_NAME="nanobot-dev"
REGION="ap-northeast-1"
ROLE_ARN="arn:aws:iam::495350830663:role/nanobot-NanobotFunctionRole-0teToGIp2YKH"

echo "🚀 Deploying nanobot to DEV environment..."

# Check if function exists
if aws lambda get-function --function-name "$FUNCTION_NAME" --region "$REGION" &>/dev/null; then
    echo "✅ Function exists, updating code..."

    # Update function code
    cd target/aarch64-unknown-linux-gnu/release
    zip -j /tmp/nanobot-dev-lambda.zip bootstrap

    aws lambda update-function-code \
        --function-name "$FUNCTION_NAME" \
        --zip-file "fileb:///tmp/nanobot-dev-lambda.zip" \
        --region "$REGION" \
        --no-cli-pager

    echo "✅ Code updated"

    # Publish new version
    VERSION=$(aws lambda publish-version \
        --function-name "$FUNCTION_NAME" \
        --region "$REGION" \
        --no-cli-pager \
        --query 'Version' \
        --output text)

    echo "✅ Published version: $VERSION"

    # Update alias
    aws lambda update-alias \
        --function-name "$FUNCTION_NAME" \
        --name live \
        --function-version "$VERSION" \
        --region "$REGION" \
        --no-cli-pager

    echo "✅ Alias 'live' updated to version $VERSION"

else
    echo "⚠️  Function does not exist, creating..."

    # Create zip
    cd target/aarch64-unknown-linux-gnu/release
    zip -j /tmp/nanobot-dev-lambda.zip bootstrap

    # Create function
    aws lambda create-function \
        --function-name "$FUNCTION_NAME" \
        --runtime provided.al2023 \
        --role "$ROLE_ARN" \
        --handler bootstrap \
        --architectures arm64 \
        --timeout 60 \
        --memory-size 512 \
        --zip-file "fileb:///tmp/nanobot-dev-lambda.zip" \
        --region "$REGION" \
        --environment "Variables={
            ENV=dev,
            RUST_LOG=nanobot=info,
            DYNAMODB_SESSIONS_TABLE=nanobot-sessions-default,
            ADMIN_SESSION_KEYS=yuki@hamada.tokyo,mail@yukihamada.jp
        }" \
        --no-cli-pager

    echo "✅ Function created"

    # Copy environment variables from prod (excluding sensitive ones that should be set separately)
    echo "📋 Copying environment variables from prod..."

    PROD_ENV=$(aws lambda get-function-configuration \
        --function-name nanobot \
        --region "$REGION" \
        --query 'Environment.Variables' \
        --output json)

    # Update environment variables
    aws lambda update-function-configuration \
        --function-name "$FUNCTION_NAME" \
        --environment "Variables=$(echo "$PROD_ENV" | jq '. + {ENV: "dev"}')" \
        --region "$REGION" \
        --no-cli-pager

    echo "✅ Environment variables copied"

    # Create live alias
    aws lambda create-alias \
        --function-name "$FUNCTION_NAME" \
        --name live \
        --function-version 1 \
        --region "$REGION" \
        --no-cli-pager

    echo "✅ Alias 'live' created"
fi

echo ""
echo "🎉 Dev deployment complete!"
echo "   Function: $FUNCTION_NAME"
echo "   Region: $REGION"
echo ""
echo "Next steps:"
echo "1. Configure API Gateway to point dev.chatweb.ai to this function"
echo "2. Update Route53 to route dev.chatweb.ai to API Gateway"
echo "3. Test: curl https://dev.chatweb.ai/health"
