#!/usr/bin/env bash
set -euo pipefail

# Fast deploy: skips SAM, directly updates Lambda function code.
# Use this for code-only changes (no infra/parameter changes).
# ~30s vs ~3min for full SAM deploy.
#
# Usage:
#   ./infra/deploy-fast.sh                # build (release) + deploy
#   ./infra/deploy-fast.sh --fast         # build (release-fast: thin LTO) + deploy (~40% faster)
#   ./infra/deploy-fast.sh --skip-build   # deploy only (reuse last build)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

FUNCTION_NAME="${LAMBDA_FUNCTION_NAME:-nanobot}"
REGION="${AWS_REGION:-ap-northeast-1}"
ALIAS_NAME="live"
SKIP_BUILD=false
CARGO_PROFILE="release"

for arg in "$@"; do
    case "$arg" in
        --skip-build) SKIP_BUILD=true ;;
        --fast) CARGO_PROFILE="release-fast" ;;
    esac
done

if [ "$CARGO_PROFILE" = "release-fast" ]; then
    BINARY="$PROJECT_ROOT/target/aarch64-unknown-linux-gnu/release-fast/bootstrap"
else
    BINARY="$PROJECT_ROOT/target/aarch64-unknown-linux-gnu/release/bootstrap"
fi
ZIP_FILE="/tmp/nanobot-lambda.zip"

echo "=== nanobot Fast Deploy ==="
echo "Function: $FUNCTION_NAME"
echo "Region:   $REGION"
echo ""

# Validate GITHUB_TOKEN is set in SSM Parameter Store
echo "--- Validating GitHub token ---"
if ! aws ssm get-parameter --name /nanobot/github-token --region "$REGION" --output text --query 'Parameter.Value' &>/dev/null; then
    echo "⚠️  GITHUB_TOKEN not found in SSM Parameter Store"
    echo "    Self-improvement features (/improve) will not work."
    echo "    To enable: aws ssm put-parameter --name /nanobot/github-token --value '<token>' --type SecureString --region $REGION"
    echo ""
    echo "    Continuing deployment without GitHub tools..."
else
    echo "✅ GITHUB_TOKEN configured"
fi
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

    echo "Profile: $CARGO_PROFILE"
    if command -v cargo-zigbuild &>/dev/null; then
        RUSTUP_TOOLCHAIN=stable \
        RUSTC="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin/rustc" \
        cargo zigbuild --manifest-path "$PROJECT_ROOT/crates/nanobot-lambda/Cargo.toml" \
            --profile "$CARGO_PROFILE" --target aarch64-unknown-linux-gnu
    elif command -v cross &>/dev/null; then
        cross build --manifest-path "$PROJECT_ROOT/crates/nanobot-lambda/Cargo.toml" \
            --profile "$CARGO_PROFILE" --target aarch64-unknown-linux-gnu
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

# 6. Health check + provider verification
echo "--- Health check ---"
HEALTH=$(curl -sf "https://chatweb.ai/health" 2>&1 || echo "FAILED")
echo "Health: $HEALTH"

# Verify LLM providers are available (prevent "No providers available" outage)
PROVIDERS=$(echo "$HEALTH" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('providers',0))" 2>/dev/null || echo "0")
STATUS=$(echo "$HEALTH" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('status',''))" 2>/dev/null || echo "")

if [ "$PROVIDERS" = "0" ] || [ "$STATUS" = "degraded" ]; then
    echo ""
    echo "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!"
    echo "!! WARNING: No LLM providers configured!            !!"
    echo "!! Users will see 'AI service unavailable' errors.  !!"
    echo "!! Set API keys in Lambda environment variables:    !!"
    echo "!!   ANTHROPIC_API_KEY, OPENAI_API_KEY, etc.        !!"
    echo "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!"
    echo ""
    echo "Rolling back to previous version..."
    PREV_VERSION=$((VERSION - 1))
    aws lambda update-alias \
        --function-name "$FUNCTION_NAME" \
        --name "$ALIAS_NAME" \
        --function-version "$PREV_VERSION" \
        --region "$REGION" \
        --output text \
        --query 'AliasArn' | xargs -I{} echo "Rolled back alias to v$PREV_VERSION: {}"
    echo ""
    echo "=== Deploy ROLLED BACK (no providers) ==="
    rm -f "$ZIP_FILE"
    exit 1
fi

echo "Providers: $PROVIDERS configured"
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
