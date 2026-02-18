#!/bin/bash
set -e

echo "🔐 Nanobot Secure Gateway Setup (Tailscale + Token + IP)"
echo "========================================================="
echo ""

# Check if Tailscale is installed
if ! command -v tailscale &> /dev/null; then
    echo "❌ Tailscale not found. Installing..."
    if [[ "$OSTYPE" == "darwin"* ]]; then
        brew install tailscale
    elif [[ "$OSTYPE" == "linux-gnu"* ]]; then
        curl -fsSL https://tailscale.com/install.sh | sh
    else
        echo "Please install Tailscale manually: https://tailscale.com/download"
        exit 1
    fi
fi

echo "✅ Tailscale installed: $(tailscale version | head -1)"
echo ""

# Check Tailscale status
if ! tailscale status &> /dev/null; then
    echo "⚠️  Tailscale is not running. Starting..."
    echo "   (This will open a browser for authentication)"
    sudo tailscale up
    echo ""
fi

# Get Tailscale IP
TAILSCALE_IP=$(tailscale ip -4 2>/dev/null || echo "unknown")
echo "📍 Your Tailscale IP: $TAILSCALE_IP"
echo ""

# Generate token if not exists
CONFIG_FILE="$HOME/.nanobot/config.json"
if [ -f "$CONFIG_FILE" ]; then
    EXISTING_TOKENS=$(jq -r '.gateway.apiTokens | length' "$CONFIG_FILE" 2>/dev/null || echo "0")
    if [ "$EXISTING_TOKENS" -gt 0 ]; then
        echo "✅ API tokens already configured ($EXISTING_TOKENS tokens)"
        jq -r '.gateway.apiTokens[]' "$CONFIG_FILE" | while read -r token; do
            echo "   - ${token:0:8}...${token: -8}"
        done
    else
        echo "🔑 Generating new API token..."
        TOKEN=$(uuidgen | tr '[:upper:]' '[:lower:]')
        echo "   Token: $TOKEN"

        # Update config.json
        jq ".gateway.apiTokens = [\"$TOKEN\"]" "$CONFIG_FILE" > /tmp/config.tmp && mv /tmp/config.tmp "$CONFIG_FILE"
        echo "   ✅ Token saved to config.json"
    fi
else
    echo "❌ Config file not found: $CONFIG_FILE"
    exit 1
fi
echo ""

# Show current config
echo "📋 Current Gateway Configuration:"
jq '.gateway' "$CONFIG_FILE"
echo ""

# Show startup command
echo "🚀 To start the secure gateway:"
echo ""
echo "   cargo build --features http-api --release"
echo "   ./target/release/chatweb gateway --http --http-port 3000 --auth"
echo ""
echo "🔗 To connect from another device:"
echo ""
echo "   # On the other device, also run: sudo tailscale up"
echo "   # Then get this server's Tailscale IP: tailscale ip -4"
echo ""
echo "   export API_TOKEN=\"\$(jq -r '.gateway.apiTokens[0]' ~/.nanobot/config.json)\""
echo "   curl -H \"Authorization: Bearer \$API_TOKEN\" \\"
echo "     http://$TAILSCALE_IP:3000/api/v1/chat \\"
echo "     -H \"Content-Type: application/json\" \\"
echo "     -d '{\"message\": \"Hello from Tailscale!\", \"session_id\": \"test\"}'"
echo ""
echo "✅ Setup complete!"
