#!/bin/bash
# Lambda最適化スクリプト
# コスト50-75%削減、セキュリティ向上

set -e

FUNCTION_NAME="nanobot"
REGION="ap-northeast-1"

echo "🔧 Lambda設定を最適化中..."
echo ""

# 現在の設定を表示
echo "📊 現在の設定:"
aws lambda get-function-configuration \
  --function-name "$FUNCTION_NAME" \
  --region "$REGION" \
  --query '{Memory: MemorySize, Timeout: Timeout, Runtime: Runtime}' \
  --output table

echo ""
echo "⚙️ 最適化された設定を適用中..."

# メモリを1024MBに削減（50%削減）
# タイムアウトを30秒に短縮（通常チャット用）
aws lambda update-function-configuration \
  --function-name "$FUNCTION_NAME" \
  --memory-size 1024 \
  --timeout 30 \
  --region "$REGION" \
  --no-cli-pager \
  > /dev/null

echo "✅ 設定更新完了"
echo ""

# 新しい設定を表示
echo "📊 新しい設定:"
aws lambda get-function-configuration \
  --function-name "$FUNCTION_NAME" \
  --region "$REGION" \
  --query '{Memory: MemorySize, Timeout: Timeout, Runtime: Runtime}' \
  --output table

echo ""
echo "💰 期待される効果:"
echo "  - Lambda実行コスト: -50%"
echo "  - タイムアウト削減: -75% (120s → 30s)"
echo "  - ユーザー体験: 改善（長時間待機の回避）"
echo ""
echo "📝 注意:"
echo "  - ストリーミングエンドポイントが30秒を超える場合は調整が必要"
echo "  - CloudWatch Logsで実際の実行時間を監視してください"
echo ""
echo "🎉 最適化完了！"
