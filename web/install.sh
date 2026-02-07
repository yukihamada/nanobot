#!/bin/sh
# chatweb.ai CLI installer
# Usage: curl -fsSL https://chatweb.ai/install.sh | sh
set -e

REPO="yukihamada/nanobot"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"

echo "  chatweb.ai CLI installer"
echo ""

# Detect OS and arch
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "$ARCH" in
  x86_64|amd64) ARCH="x86_64" ;;
  aarch64|arm64) ARCH="aarch64" ;;
  *) echo "Unsupported architecture: $ARCH"; exit 1 ;;
esac

case "$OS" in
  linux) TARGET="${ARCH}-unknown-linux-gnu" ;;
  darwin) TARGET="${ARCH}-apple-darwin" ;;
  *) echo "Unsupported OS: $OS"; exit 1 ;;
esac

# Try to download prebuilt binary from GitHub releases
LATEST=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" 2>/dev/null | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/' || echo "")

if [ -n "$LATEST" ]; then
  ASSET_URL="https://github.com/${REPO}/releases/download/${LATEST}/nanobot-${TARGET}.tar.gz"
  echo "Downloading nanobot ${LATEST} for ${TARGET}..."

  TMP=$(mktemp -d)
  if curl -fsSL "$ASSET_URL" -o "$TMP/nanobot.tar.gz" 2>/dev/null; then
    mkdir -p "$INSTALL_DIR"
    tar xzf "$TMP/nanobot.tar.gz" -C "$TMP"
    mv "$TMP/nanobot" "$INSTALL_DIR/nanobot"
    chmod +x "$INSTALL_DIR/nanobot"
    rm -rf "$TMP"

    echo ""
    echo "Installed nanobot to $INSTALL_DIR/nanobot"
    ensure_path
    exit 0
  fi
  rm -rf "$TMP"
  echo "Prebuilt binary not available, falling back to cargo install..."
fi

# Fall back to cargo install
if command -v cargo >/dev/null 2>&1; then
  echo "Installing via cargo..."
  cargo install --git "https://github.com/${REPO}" --branch feat/ai-webhooks nanobot
  echo ""
  echo "Installed! Run: nanobot chat \"Hello!\""
  exit 0
fi

# No cargo, try to install Rust first
echo "Rust not found. Installing Rust first..."
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
. "$HOME/.cargo/env"
echo "Installing nanobot..."
cargo install --git "https://github.com/${REPO}" --branch feat/ai-webhooks nanobot
echo ""
echo "Installed! Run: nanobot chat \"Hello!\""

ensure_path() {
  case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *)
      echo ""
      echo "Add to your PATH:"
      echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
      ;;
  esac
}
