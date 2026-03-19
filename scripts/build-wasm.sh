#!/bin/bash
set -euo pipefail

# Build WASM frontend for chatweb.ai
# Usage: ./scripts/build-wasm.sh

cd "$(dirname "$0")/.."

echo "Building WASM frontend..."
cd crates/chatweb-frontend
trunk build
cd ../..

# Optimize with system wasm-opt if available (optional)
if command -v wasm-opt &>/dev/null; then
    for f in dist/*_bg.wasm; do
        [ -f "$f" ] || continue
        echo "Optimizing $f..."
        wasm-opt \
            --enable-bulk-memory \
            --enable-nontrapping-float-to-int \
            --enable-sign-ext \
            --enable-mutable-globals \
            -Oz "$f" -o "$f.opt" && mv "$f.opt" "$f"
    done
fi

echo "Build complete:"
ls -lh dist/
