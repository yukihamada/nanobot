#!/usr/bin/env bash
set -euo pipefail

STACK_NAME="${STACK_NAME:-nanobot}"
REGION="${AWS_REGION:-ap-northeast-1}"

echo "=== nanobot webhook setup ==="

# Get API URL from CloudFormation outputs
API_URL=$(aws cloudformation describe-stacks \
    --stack-name "$STACK_NAME" \
    --region "$REGION" \
    --query 'Stacks[0].Outputs[?OutputKey==`ApiUrl`].OutputValue' \
    --output text)

if [ -z "$API_URL" ]; then
    echo "ERROR: Could not find ApiUrl output from stack $STACK_NAME"
    exit 1
fi

echo "API URL: $API_URL"
echo ""

# 1. Telegram webhook setup
TELEGRAM_BOT_TOKEN="${TELEGRAM_BOT_TOKEN:-}"

if [ -n "$TELEGRAM_BOT_TOKEN" ]; then
    TELEGRAM_WEBHOOK_URL="$API_URL/webhooks/telegram"
    echo "--- Setting Telegram webhook ---"
    echo "URL: $TELEGRAM_WEBHOOK_URL"

    RESULT=$(curl -s "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/setWebhook" \
        -H "Content-Type: application/json" \
        -d "{\"url\": \"${TELEGRAM_WEBHOOK_URL}\", \"allowed_updates\": [\"message\"]}")

    OK=$(echo "$RESULT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('ok', False))" 2>/dev/null || echo "false")

    if [ "$OK" = "True" ]; then
        echo "Telegram webhook set successfully!"
    else
        echo "ERROR: Failed to set Telegram webhook"
        echo "$RESULT"
        exit 1
    fi
    echo ""
else
    echo "TELEGRAM_BOT_TOKEN not set, skipping Telegram webhook."
    echo ""
fi

# 2. LINE webhook — manual setup required
LINE_WEBHOOK_URL="$API_URL/webhooks/line"
echo "--- LINE webhook setup ---"
echo "LINE requires manual configuration in the LINE Developer Console."
echo ""
echo "  Webhook URL: $LINE_WEBHOOK_URL"
echo ""
echo "Steps:"
echo "  1. Go to https://developers.line.biz/console/"
echo "  2. Select your channel"
echo "  3. Go to Messaging API > Webhook settings"
echo "  4. Set Webhook URL to the URL above"
echo "  5. Enable 'Use webhook'"
echo "  6. Click 'Verify' to test the connection"
echo ""

# 3. Health check
echo "--- Health check ---"
HEALTH_URL="$API_URL/health"
echo "Checking $HEALTH_URL ..."

HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" "$HEALTH_URL" || echo "000")

if [ "$HTTP_CODE" = "200" ]; then
    echo "Health check passed! (HTTP $HTTP_CODE)"
    curl -s "$HEALTH_URL" | python3 -m json.tool 2>/dev/null || true
else
    echo "WARNING: Health check returned HTTP $HTTP_CODE"
    echo "The Lambda function may still be cold-starting. Try again in a few seconds."
fi

echo ""
echo "=== Setup complete ==="
