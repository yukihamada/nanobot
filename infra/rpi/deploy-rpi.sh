#!/usr/bin/env bash
set -euo pipefail

# deploy-rpi.sh — Build and deploy chatweb to Raspberry Pi 4
#
# Usage:
#   ./infra/rpi/deploy-rpi.sh                # Build + deploy
#   ./infra/rpi/deploy-rpi.sh --fast         # Fast build + deploy
#   ./infra/rpi/deploy-rpi.sh --skip-build   # Deploy only (reuse last build)
#
# Environment:
#   RPI_HOST    — Pi hostname/IP (default: chatweb-pi.local)
#   RPI_USER    — SSH user (default: yuki)
#   SSH_KEY     — SSH key path (default: ~/.ssh/id_ed25519_rpi)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

RPI_HOST="${RPI_HOST:-chatweb-pi.local}"
RPI_USER="${RPI_USER:-yuki}"
SSH_KEY="${SSH_KEY:-$HOME/.ssh/id_ed25519_rpi}"

SKIP_BUILD=false
BUILD_ARGS=()

for arg in "$@"; do
    case "$arg" in
        --skip-build) SKIP_BUILD=true ;;
        --fast) BUILD_ARGS+=("--fast") ;;
    esac
done

# Determine binary path based on build profile
if [[ " ${BUILD_ARGS[*]:-} " =~ " --fast " ]]; then
    BINARY="$PROJECT_ROOT/target/aarch64-unknown-linux-musl/release-fast/chatweb"
else
    BINARY="$PROJECT_ROOT/target/aarch64-unknown-linux-musl/release/chatweb"
fi

SSH_CMD="ssh -i $SSH_KEY -o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new"
SCP_CMD="scp -i $SSH_KEY -o ConnectTimeout=10"

echo "=== chatweb RPi4 Deploy ==="
echo "Host: $RPI_USER@$RPI_HOST"
echo ""

# --- 1. Build ---
if [ "$SKIP_BUILD" = true ]; then
    echo "--- Skipping build (--skip-build) ---"
    if [ ! -f "$BINARY" ]; then
        echo "ERROR: No binary found at $BINARY"
        echo "Run without --skip-build first."
        exit 1
    fi
else
    echo "--- Building ---"
    "$SCRIPT_DIR/build-rpi.sh" "${BUILD_ARGS[@]+"${BUILD_ARGS[@]}"}"
    echo ""
fi

BINARY_SIZE=$(du -h "$BINARY" | cut -f1)
echo "Binary: $BINARY ($BINARY_SIZE)"
echo ""

# --- 2. Check connectivity ---
echo "--- Checking connectivity ---"
if ! $SSH_CMD "$RPI_USER@$RPI_HOST" "echo ok" &>/dev/null; then
    echo "ERROR: Cannot connect to $RPI_USER@$RPI_HOST"
    echo ""
    echo "Troubleshooting:"
    echo "  1. Is the Pi powered on and connected to WiFi?"
    echo "  2. Try: ping $RPI_HOST"
    echo "  3. Try: ssh -i $SSH_KEY $RPI_USER@$RPI_HOST"
    exit 1
fi
echo "Connected to $RPI_HOST"
echo ""

# --- 3. Upload binary ---
echo "--- Uploading binary ---"
START_UPLOAD=$(date +%s)
$SCP_CMD "$BINARY" "$RPI_USER@$RPI_HOST:/tmp/chatweb.new"
END_UPLOAD=$(date +%s)
echo "Upload time: $((END_UPLOAD - START_UPLOAD))s"

# --- 4. Upload boot-sound script ---
BOOT_SOUND="$SCRIPT_DIR/boot-sound.sh"
if [ -f "$BOOT_SOUND" ]; then
    echo "--- Uploading boot-sound.sh ---"
    $SCP_CMD "$BOOT_SOUND" "$RPI_USER@$RPI_HOST:/tmp/boot-sound.sh"
    $SSH_CMD "$RPI_USER@$RPI_HOST" "sudo mv /tmp/boot-sound.sh /opt/chatweb/boot-sound.sh && sudo chmod +x /opt/chatweb/boot-sound.sh"
    echo "Updated boot-sound.sh"
fi

# --- 5. Upload .env if it exists ---
ENV_FILE="$SCRIPT_DIR/.env.rpi"
if [ -f "$ENV_FILE" ]; then
    echo "--- Uploading .env ---"
    $SCP_CMD "$ENV_FILE" "$RPI_USER@$RPI_HOST:/tmp/chatweb.env.new"
    $SSH_CMD "$RPI_USER@$RPI_HOST" "sudo mv /tmp/chatweb.env.new /opt/chatweb/.env && sudo chown chatweb:chatweb /opt/chatweb/.env && sudo chmod 600 /opt/chatweb/.env"
    echo "Updated /opt/chatweb/.env"
else
    echo "--- No .env.rpi found, skipping env upload ---"
    echo "  Create $ENV_FILE from .env.rpi.example to configure API keys."
fi

# --- 6. Install binary and restart service ---
echo "--- Installing and restarting ---"
$SSH_CMD "$RPI_USER@$RPI_HOST" << 'REMOTEEOF'
set -euo pipefail

# Move new binary into place
sudo mv /tmp/chatweb.new /opt/chatweb/chatweb
sudo chown chatweb:chatweb /opt/chatweb/chatweb
sudo chmod +x /opt/chatweb/chatweb

# Show version
/opt/chatweb/chatweb --version 2>/dev/null || echo "(version check skipped)"

# Restart service
sudo systemctl restart chatweb
echo "Service restarted"

# Wait for startup
sleep 2
sudo systemctl is-active chatweb && echo "Service is running" || echo "WARNING: Service not active"
REMOTEEOF
echo ""

# --- 7. Health check ---
echo "--- Health check ---"
HEALTH="FAILED"
for i in 1 2 3 4 5; do
    sleep 2
    HEALTH=$(curl -sf "http://$RPI_HOST:3000/health" 2>/dev/null || echo "FAILED")
    if [ "$HEALTH" != "FAILED" ]; then
        break
    fi
    echo "  Attempt $i/5 failed, retrying..."
done

if [ "$HEALTH" = "FAILED" ]; then
    echo "WARNING: Health check failed"
    echo ""
    echo "Checking service logs:"
    $SSH_CMD "$RPI_USER@$RPI_HOST" "sudo journalctl -u chatweb -n 20 --no-pager" || true
    exit 1
fi

echo "Health: $HEALTH"
echo ""

# --- 8. Summary ---
echo "=== Deploy complete ==="
echo "URL: http://$RPI_HOST:3000"
echo ""
echo "Useful commands:"
echo "  ssh -i $SSH_KEY $RPI_USER@$RPI_HOST"
echo "  curl http://$RPI_HOST:3000/health"
echo "  ssh -i $SSH_KEY $RPI_USER@$RPI_HOST 'sudo journalctl -u chatweb -f'"
