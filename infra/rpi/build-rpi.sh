#!/usr/bin/env bash
set -euo pipefail

# build-rpi.sh — Cross-compile chatweb for Raspberry Pi 4 (ARM64)
#
# Builds the root binary (chatweb gateway) for aarch64-unknown-linux-musl
# with Cortex-A72 CPU optimizations (RPi4 SoC).
#
# Usage:
#   ./infra/rpi/build-rpi.sh           # Full release build (fat LTO)
#   ./infra/rpi/build-rpi.sh --fast    # Fast build (thin LTO, ~40% faster compile)
#
# Prerequisites:
#   cargo install cargo-zigbuild
#   rustup target add aarch64-unknown-linux-musl

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

CARGO_PROFILE="release"
for arg in "$@"; do
    case "$arg" in
        --fast) CARGO_PROFILE="release-fast" ;;
    esac
done

if [ "$CARGO_PROFILE" = "release-fast" ]; then
    BINARY="$PROJECT_ROOT/target/aarch64-unknown-linux-musl/release-fast/chatweb"
else
    BINARY="$PROJECT_ROOT/target/aarch64-unknown-linux-musl/release/chatweb"
fi

echo "=== chatweb RPi4 Build ==="
echo "Profile: $CARGO_PROFILE"
echo "Target:  aarch64-unknown-linux-musl (Cortex-A72)"
echo ""

START_BUILD=$(date +%s)

# Raspberry Pi 4 uses Cortex-A72 (ARMv8-A)
# - codegen-units=1: Better optimization
export RUSTFLAGS="-C target-cpu=cortex-a72 -C codegen-units=1 ${RUSTFLAGS:-}"
echo "RUSTFLAGS: $RUSTFLAGS"

# Use sccache if available
if command -v sccache &>/dev/null; then
    export RUSTC_WRAPPER=sccache
    echo "Using sccache"
fi

# Parallel jobs
JOBS=$(sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo "4")
export CARGO_BUILD_JOBS=$JOBS
echo "Parallel jobs: $JOBS"
echo ""

# Build the root binary (not the lambda binary)
# Features: http-api (for gateway mode), no dynamodb-backend (uses file-backend)
if command -v cargo-zigbuild &>/dev/null; then
    echo "--- Building with cargo-zigbuild ---"
    RUSTUP_TOOLCHAIN=stable \
    RUSTC="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin/rustc" \
    cargo zigbuild --manifest-path "$PROJECT_ROOT/Cargo.toml" \
        --features http-api \
        --profile "$CARGO_PROFILE" \
        --target aarch64-unknown-linux-musl \
        --bin chatweb \
        -j "$JOBS"
elif command -v cross &>/dev/null; then
    echo "--- Building with cross ---"
    cross build --manifest-path "$PROJECT_ROOT/Cargo.toml" \
        --features http-api \
        --profile "$CARGO_PROFILE" \
        --target aarch64-unknown-linux-musl \
        --bin chatweb \
        -j "$JOBS"
else
    echo "ERROR: Neither cargo-zigbuild nor cross found."
    echo "Install with: cargo install cargo-zigbuild"
    exit 1
fi

END_BUILD=$(date +%s)

echo ""
echo "--- Build complete ---"
echo "Binary: $BINARY"

if [ -f "$BINARY" ]; then
    BINARY_SIZE=$(du -h "$BINARY" | cut -f1)
    echo "Size:   $BINARY_SIZE"
    file "$BINARY" | sed 's/.*: /Type:   /'
else
    echo "ERROR: Binary not found at $BINARY"
    exit 1
fi

echo "Time:   $((END_BUILD - START_BUILD))s"

# Show sccache stats if available
if command -v sccache &>/dev/null; then
    echo ""
    sccache --show-stats 2>/dev/null | grep -E "(Hits|Misses|Cache size)" || true
fi
