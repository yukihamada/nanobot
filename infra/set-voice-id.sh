#!/usr/bin/env bash
set -euo pipefail

# Set ElevenLabs voice ID for Yuki
# Usage: ./infra/set-voice-id.sh <VOICE_ID>

if [ $# -eq 0 ]; then
    echo "Usage: $0 <ELEVENLABS_VOICE_ID>"
    echo ""
    echo "Example:"
    echo "  $0 pNInz6obpgDQGcFmaJgB"
    echo ""
    echo "To find your voice ID:"
    echo "  1. Go to https://elevenlabs.io/app/voice-library"
    echo "  2. Select your voice"
    echo "  3. Copy the voice ID from the URL or settings"
    exit 1
fi

VOICE_ID="$1"
FUNCTION_NAME="${LAMBDA_FUNCTION_NAME:-nanobot}"
REGION="${AWS_REGION:-ap-northeast-1}"

echo "=== Setting ElevenLabs Voice ID ==="
echo "Function: $FUNCTION_NAME"
echo "Region:   $REGION"
echo "Voice ID: $VOICE_ID"
echo ""

# Get current environment variables
echo "--- Fetching current environment ---"
CURRENT_ENV=$(aws lambda get-function-configuration \
    --function-name "$FUNCTION_NAME" \
    --region "$REGION" \
    --query 'Environment.Variables' \
    --output json)

# Add or update ELEVENLABS_YUKI_VOICE_ID
echo "--- Updating environment ---"
UPDATED_ENV=$(echo "$CURRENT_ENV" | jq --arg vid "$VOICE_ID" '. + {"ELEVENLABS_YUKI_VOICE_ID": $vid}')

aws lambda update-function-configuration \
    --function-name "$FUNCTION_NAME" \
    --region "$REGION" \
    --environment "Variables=$UPDATED_ENV" \
    --output text \
    --query 'FunctionArn'

echo ""
echo "✓ Voice ID set successfully!"
echo ""
echo "To test:"
echo "  1. Go to https://chatweb.ai"
echo "  2. Click the logo or AI avatar"
echo "  3. Follow the welcome audio flow"
echo ""
