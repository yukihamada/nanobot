#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TARGET_DIR="$PROJECT_ROOT/target/lambda/bootstrap"

STACK_NAME="${STACK_NAME:-nanobot}"
REGION="${AWS_REGION:-ap-northeast-1}"
S3_BUCKET="${SAM_S3_BUCKET:-}"

echo "=== nanobot Lambda deploy ==="
echo "Region: $REGION"
echo "Stack:  $STACK_NAME"

# 1. Cross-compile for ARM64 Linux
echo ""
echo "--- Building for aarch64-unknown-linux-gnu ---"

if command -v cargo-zigbuild &>/dev/null; then
    echo "Using cargo-zigbuild"
    cargo zigbuild --manifest-path "$PROJECT_ROOT/crates/nanobot-lambda/Cargo.toml" \
        --release --target aarch64-unknown-linux-gnu
elif command -v cross &>/dev/null; then
    echo "Using cross"
    cross build --manifest-path "$PROJECT_ROOT/crates/nanobot-lambda/Cargo.toml" \
        --release --target aarch64-unknown-linux-gnu
else
    echo "ERROR: Neither cargo-zigbuild nor cross found."
    echo "Install one:"
    echo "  cargo install cargo-zigbuild && brew install zig"
    echo "  cargo install cross"
    exit 1
fi

# 2. Copy binary to Lambda bootstrap location
echo ""
echo "--- Preparing bootstrap binary ---"
mkdir -p "$TARGET_DIR"
cp "$PROJECT_ROOT/target/aarch64-unknown-linux-gnu/release/bootstrap" "$TARGET_DIR/bootstrap"
chmod +x "$TARGET_DIR/bootstrap"

BINARY_SIZE=$(du -h "$TARGET_DIR/bootstrap" | cut -f1)
echo "Binary size: $BINARY_SIZE"

# 3. SAM deploy
echo ""
echo "--- Deploying with SAM ---"

SAM_ARGS=(
    --template-file "$SCRIPT_DIR/template.yaml"
    --stack-name "$STACK_NAME"
    --region "$REGION"
    --capabilities CAPABILITY_IAM
    --resolve-s3
    --no-confirm-changeset
)

if [ -n "$S3_BUCKET" ]; then
    SAM_ARGS+=(--s3-bucket "$S3_BUCKET")
fi

# Pass parameter overrides from environment variables
PARAM_OVERRIDES=""

[ -n "${ANTHROPIC_API_KEY:-}" ] && PARAM_OVERRIDES+=" AnthropicApiKey=$ANTHROPIC_API_KEY"
[ -n "${TELEGRAM_BOT_TOKEN:-}" ] && PARAM_OVERRIDES+=" TelegramBotToken=$TELEGRAM_BOT_TOKEN"
[ -n "${TELEGRAM_ALLOW_FROM:-}" ] && PARAM_OVERRIDES+=" TelegramAllowFrom=$TELEGRAM_ALLOW_FROM"
[ -n "${LINE_CHANNEL_SECRET:-}" ] && PARAM_OVERRIDES+=" LineChannelSecret=$LINE_CHANNEL_SECRET"
[ -n "${LINE_CHANNEL_ACCESS_TOKEN:-}" ] && PARAM_OVERRIDES+=" LineChannelAccessToken=$LINE_CHANNEL_ACCESS_TOKEN"
[ -n "${LINE_ALLOW_FROM:-}" ] && PARAM_OVERRIDES+=" LineAllowFrom=$LINE_ALLOW_FROM"
[ -n "${NANOBOT_MODEL:-}" ] && PARAM_OVERRIDES+=" NanobotModel=$NANOBOT_MODEL"
[ -n "${NANOBOT_TENANT_ID:-}" ] && PARAM_OVERRIDES+=" TenantId=$NANOBOT_TENANT_ID"

if [ -n "$PARAM_OVERRIDES" ]; then
    SAM_ARGS+=(--parameter-overrides "$PARAM_OVERRIDES")
fi

sam deploy "${SAM_ARGS[@]}"

# 4. Show outputs
echo ""
echo "=== Deploy complete ==="
echo ""
aws cloudformation describe-stacks \
    --stack-name "$STACK_NAME" \
    --region "$REGION" \
    --query 'Stacks[0].Outputs[*].[OutputKey,OutputValue]' \
    --output table
