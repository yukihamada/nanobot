#!/bin/bash
# Deploy to development environment with cost optimizations

set -e

echo "🔧 Deploying to DEV environment..."
echo "   - Memory: 512MB (vs 2048MB prod)"
echo "   - Timeout: 60s (vs 120s prod)"
echo "   - Log retention: 7 days (vs 30 days prod)"
echo ""

sam deploy \
  --template-file template.yaml \
  --stack-name nanobot-dev \
  --capabilities CAPABILITY_IAM \
  --parameter-overrides \
    Environment=dev \
    TenantId=dev \
  --resolve-s3 \
  --region ap-northeast-1

echo ""
echo "✅ Dev deployment complete!"
echo "   API URL: https://dev.chatweb.ai"
echo "   Logs: /aws/lambda/nanobot-dev"
