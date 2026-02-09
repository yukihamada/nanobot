#!/usr/bin/env bash
set -euo pipefail

# Fast deploy: skips SAM, directly updates Lambda function code.
# Use this for code-only changes (no infra/parameter changes).
# ~30s vs ~3min for full SAM deploy.
#
# Usage:
#   ./infra/deploy-fast.sh           # build + deploy
#   ./infra/deploy-fast.sh --skip-build  # deploy only (reuse last build)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

FUNCTION_NAME="${LAMBDA_FUNCTION_NAME:-nanobot}"
REGION="${AWS_REGION:-ap-northeast-1}"
ALIAS_NAME="live"
SKIP_BUILD=false

for arg in "$@"; do
    case "$arg" in
        --skip-build) SKIP_BUILD=true ;;
    esac
done

BINARY="$PROJECT_ROOT/target/aarch64-unknown-linux-gnu/release/bootstrap"
ZIP_FILE="/tmp/nanobot-lambda.zip"

echo "=== nanobot Fast Deploy ==="
echo "Function: $FUNCTION_NAME"
echo "Region:   $REGION"
echo ""

# 1. Build (unless --skip-build)
if [ "$SKIP_BUILD" = true ]; then
    echo "--- Skipping build (--skip-build) ---"
    if [ ! -f "$BINARY" ]; then
        echo "ERROR: No binary found at $BINARY"
        echo "Run without --skip-build first."
        exit 1
    fi
else
    echo "--- Building for aarch64-unknown-linux-gnu ---"
    START_BUILD=$(date +%s)

    if command -v cargo-zigbuild &>/dev/null; then
        RUSTUP_TOOLCHAIN=stable \
        RUSTC="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin/rustc" \
        cargo zigbuild --manifest-path "$PROJECT_ROOT/crates/nanobot-lambda/Cargo.toml" \
            --release --target aarch64-unknown-linux-gnu
    elif command -v cross &>/dev/null; then
        cross build --manifest-path "$PROJECT_ROOT/crates/nanobot-lambda/Cargo.toml" \
            --release --target aarch64-unknown-linux-gnu
    else
        echo "ERROR: Neither cargo-zigbuild nor cross found."
        exit 1
    fi

    END_BUILD=$(date +%s)
    echo "Build time: $((END_BUILD - START_BUILD))s"
fi

BINARY_SIZE=$(du -h "$BINARY" | cut -f1)
echo "Binary size: $BINARY_SIZE"
echo ""

# 2. Create zip
echo "--- Creating zip ---"
START_ZIP=$(date +%s)
TMPDIR=$(mktemp -d)
cp "$BINARY" "$TMPDIR/bootstrap"
chmod +x "$TMPDIR/bootstrap"
zip -j "$ZIP_FILE" "$TMPDIR/bootstrap"
rm -rf "$TMPDIR"
END_ZIP=$(date +%s)
ZIP_SIZE=$(du -h "$ZIP_FILE" | cut -f1)
echo "Zip size: $ZIP_SIZE (${END_ZIP}s - ${START_ZIP}s = $((END_ZIP - START_ZIP))s)"
echo ""

# 3. Update Lambda function code
echo "--- Updating Lambda function code ---"
START_DEPLOY=$(date +%s)

aws lambda update-function-code \
    --function-name "$FUNCTION_NAME" \
    --zip-file "fileb://$ZIP_FILE" \
    --region "$REGION" \
    --output text \
    --query 'CodeSize' | xargs -I{} echo "Deployed code size: {} bytes"

# Wait for update to complete
echo "Waiting for function update..."
aws lambda wait function-updated \
    --function-name "$FUNCTION_NAME" \
    --region "$REGION"

# 4. Publish new version
echo "--- Publishing new version ---"
VERSION=$(aws lambda publish-version \
    --function-name "$FUNCTION_NAME" \
    --region "$REGION" \
    --output text \
    --query 'Version')
echo "Published version: v$VERSION"

# 5. Update live alias
echo "--- Updating '$ALIAS_NAME' alias → v$VERSION ---"
aws lambda update-alias \
    --function-name "$FUNCTION_NAME" \
    --name "$ALIAS_NAME" \
    --function-version "$VERSION" \
    --region "$REGION" \
    --output text \
    --query 'AliasArn' | xargs -I{} echo "Alias ARN: {}"

END_DEPLOY=$(date +%s)
echo ""

# 6. Health check
echo "--- Health check ---"
HEALTH=$(curl -sf "https://chatweb.ai/health" 2>&1 || echo "FAILED")
echo "Health: $HEALTH"
echo ""

# 7. Summary
DEPLOY_ONLY=$((END_DEPLOY - START_ZIP))
echo "=== Deploy complete ==="
echo "Version:     v$VERSION"
echo "Deploy time: ${DEPLOY_ONLY}s (excluding build)"
if [ "$SKIP_BUILD" = false ] && [ -n "${START_BUILD:-}" ]; then
    echo "Total time:  $((END_DEPLOY - START_BUILD))s (including build)"
fi

# Cleanup
rm -f "$ZIP_FILE"
