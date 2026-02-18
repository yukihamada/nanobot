#!/bin/bash
# Deploy to production environment

set -e

echo "🚀 Deploying to PRODUCTION environment..."
echo "   - Memory: 2048MB"
echo "   - Timeout: 120s"
echo "   - Log retention: 30 days"
echo ""

sam deploy \
  --template-file template.yaml \
  --stack-name nanobot-prod \
  --capabilities CAPABILITY_IAM \
  --parameter-overrides \
    Environment=prod \
    TenantId=default \
  --resolve-s3 \
  --region ap-northeast-1

echo ""
echo "✅ Production deployment complete!"
echo "   API URL: https://chatweb.ai"
echo "   Logs: /aws/lambda/nanobot-prod"
