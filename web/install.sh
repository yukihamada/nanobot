#!/bin/sh
# chatweb.ai CLI installer
# Usage: curl -fsSL https://chatweb.ai/install.sh | sh
set -e

REPO="yukihamada/nanobot"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
BINARY_NAME="nanobot"

# Colors (disable if not a terminal)
if [ -t 1 ]; then
  BOLD="\033[1m"
  GREEN="\033[32m"
  YELLOW="\033[33m"
  RED="\033[31m"
  CYAN="\033[36m"
  RESET="\033[0m"
else
  BOLD="" GREEN="" YELLOW="" RED="" CYAN="" RESET=""
fi

info()  { printf "${CYAN}info${RESET}  %s\n" "$1"; }
warn()  { printf "${YELLOW}warn${RESET}  %s\n" "$1"; }
error() { printf "${RED}error${RESET} %s\n" "$1"; exit 1; }

# --- Windows detection ---
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*|Windows_NT)
    echo ""
    echo "${BOLD}chatweb.ai CLI${RESET}"
    echo ""
    warn "Windows detected. Please use WSL (Windows Subsystem for Linux):"
    echo ""
    echo "  1. Install WSL:  wsl --install"
    echo "  2. Open WSL terminal"
    echo "  3. Run:  curl -fsSL https://chatweb.ai/install.sh | sh"
    echo ""
    exit 1
    ;;
esac

# --- OS / Arch detection ---
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "$ARCH" in
  x86_64|amd64)   ARCH="x86_64"  ;;
  aarch64|arm64)   ARCH="aarch64" ;;
  *) error "Unsupported architecture: $ARCH" ;;
esac

case "$OS" in
  linux)  TARGET="${ARCH}-unknown-linux-gnu" ;;
  darwin) TARGET="${ARCH}-apple-darwin" ;;
  *) error "Unsupported OS: $OS" ;;
esac

echo ""
printf "${BOLD}  chatweb.ai CLI installer${RESET}\n"
echo ""
info "Detected: ${OS} / ${ARCH} (${TARGET})"

# --- Resolve version ---
if [ -z "$VERSION" ]; then
  info "Fetching latest release..."
  VERSION=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" 2>/dev/null \
    | grep '"tag_name"' | head -1 \
    | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/' || echo "")
fi

if [ -z "$VERSION" ]; then
  error "Could not determine latest version. Check https://github.com/${REPO}/releases"
fi

info "Version: ${VERSION}"

# --- Download ---
ASSET="nanobot-${TARGET}.tar.gz"
GITHUB_URL="https://github.com/${REPO}/releases/download/${VERSION}/${ASSET}"
FALLBACK_URL="https://chatweb.ai/dl/${ASSET}"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

download_ok=false

info "Downloading ${ASSET}..."
if curl -fsSL "$GITHUB_URL" -o "$TMP/${ASSET}" 2>/dev/null; then
  download_ok=true
else
  info "GitHub download failed, trying fallback..."
  if curl -fsSL -L "$FALLBACK_URL" -o "$TMP/${ASSET}" 2>/dev/null; then
    download_ok=true
  fi
fi

if [ "$download_ok" = false ]; then
  echo ""
  warn "Prebuilt binary not available for ${TARGET}."
  echo ""
  echo "  You can build from source with Rust:"
  echo "    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
  echo "    cargo install --git https://github.com/${REPO} chatweb"
  echo ""
  exit 1
fi

# --- Install ---
mkdir -p "$INSTALL_DIR"
tar xzf "$TMP/${ASSET}" -C "$TMP"
mv "$TMP/${BINARY_NAME}" "$INSTALL_DIR/chatweb"
chmod +x "$INSTALL_DIR/chatweb"

echo ""
printf "${GREEN}${BOLD}  Installed chatweb to ${INSTALL_DIR}/chatweb${RESET}\n"

# --- PATH setup ---
case ":$PATH:" in
  *":${INSTALL_DIR}:"*)
    # Already in PATH
    ;;
  *)
    if [ "$NANOBOT_NO_MODIFY_PATH" = "1" ]; then
      echo ""
      warn "Add to your PATH manually:"
      echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
    else
      # Detect shell config file
      SHELL_NAME="$(basename "${SHELL:-/bin/sh}")"
      case "$SHELL_NAME" in
        zsh)  RC_FILE="$HOME/.zshrc" ;;
        bash) RC_FILE="$HOME/.bashrc" ;;
        fish) RC_FILE="$HOME/.config/fish/config.fish" ;;
        *)    RC_FILE="" ;;
      esac

      if [ -n "$RC_FILE" ]; then
        if [ -f "$RC_FILE" ] && grep -q "$INSTALL_DIR" "$RC_FILE" 2>/dev/null; then
          : # Already configured
        else
          if [ "$SHELL_NAME" = "fish" ]; then
            echo "set -gx PATH \"${INSTALL_DIR}\" \$PATH" >> "$RC_FILE"
          else
            echo "export PATH=\"${INSTALL_DIR}:\$PATH\"" >> "$RC_FILE"
          fi
          info "Added ${INSTALL_DIR} to PATH in ${RC_FILE}"
        fi
      fi

      # Also export for current session
      export PATH="${INSTALL_DIR}:$PATH"
    fi
    ;;
esac

# --- Done ---
echo ""
printf "${BOLD}Get started:${RESET}\n"
echo "  chatweb chat \"Hello!\""
echo ""
