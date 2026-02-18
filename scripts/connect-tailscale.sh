#!/bin/bash
set -e

echo "🔗 Nanobot Tailscale Client Connection"
echo "======================================="
echo ""

# Check arguments
if [ $# -lt 1 ]; then
    echo "Usage: $0 <server-tailscale-ip> [api-token]"
    echo ""
    echo "Example:"
    echo "  $0 100.64.1.5"
    echo "  $0 100.64.1.5 a34704a8-9c52-48d1-8b5c-f0ac6045ca18"
    echo ""
    exit 1
fi

SERVER_IP="$1"
API_TOKEN="${2:-}"

# Check if Tailscale is running
if ! tailscale status &> /dev/null; then
    echo "❌ Tailscale is not running. Please start it first:"
    echo "   sudo tailscale up"
    exit 1
fi

# Get local Tailscale IP
LOCAL_IP=$(tailscale ip -4 2>/dev/null || echo "unknown")
echo "📍 Your Tailscale IP: $LOCAL_IP"
echo "🎯 Server IP: $SERVER_IP"
echo ""

# Get or prompt for API token
if [ -z "$API_TOKEN" ]; then
    echo "🔑 Enter API token (or press Enter to use default from config.json):"
    read -r TOKEN_INPUT

    if [ -n "$TOKEN_INPUT" ]; then
        API_TOKEN="$TOKEN_INPUT"
    else
        # Try to read from local config
        CONFIG_FILE="$HOME/.nanobot/config.json"
        if [ -f "$CONFIG_FILE" ]; then
            API_TOKEN=$(jq -r '.gateway.apiTokens[0]' "$CONFIG_FILE" 2>/dev/null || echo "")
        fi
    fi
fi

if [ -z "$API_TOKEN" ]; then
    echo "❌ No API token provided"
    exit 1
fi

echo "✅ Using token: ${API_TOKEN:0:8}...${API_TOKEN: -8}"
echo ""

# Test connection
echo "🧪 Testing connection..."
RESPONSE=$(curl -s -w "\n%{http_code}" \
    -H "Authorization: Bearer $API_TOKEN" \
    -H "Content-Type: application/json" \
    -X POST \
    "http://$SERVER_IP:3000/api/v1/chat" \
    -d '{"message": "Hello from Tailscale!", "session_id": "tailscale-test"}')

HTTP_CODE=$(echo "$RESPONSE" | tail -1)
BODY=$(echo "$RESPONSE" | head -n -1)

if [ "$HTTP_CODE" = "200" ]; then
    echo "✅ Connection successful!"
    echo ""
    echo "Response:"
    echo "$BODY" | jq -r '.response' 2>/dev/null || echo "$BODY"
    echo ""
    echo "🎉 You can now use the nanobot gateway securely via Tailscale!"
    echo ""
    echo "Example usage:"
    echo "  export NANOBOT_SERVER=\"http://$SERVER_IP:3000\""
    echo "  export NANOBOT_TOKEN=\"$API_TOKEN\""
    echo ""
    echo "  curl -H \"Authorization: Bearer \$NANOBOT_TOKEN\" \\"
    echo "    \$NANOBOT_SERVER/api/v1/chat \\"
    echo "    -H \"Content-Type: application/json\" \\"
    echo "    -d '{\"message\": \"Your query here\", \"session_id\": \"my-session\"}'"
else
    echo "❌ Connection failed (HTTP $HTTP_CODE)"
    echo ""
    echo "Response:"
    echo "$BODY"
    echo ""
    echo "Troubleshooting:"
    echo "  1. Check if server is running: tailscale ping $SERVER_IP"
    echo "  2. Verify API token is correct"
    echo "  3. Check server logs for errors"
fi
