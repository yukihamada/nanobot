#!/bin/bash
set -euo pipefail

# 自動サブドメイン生成＆デプロイ
# GitHubからコードを取得→コンパイル→デプロイ→ランダムサブドメイン割り当て

GITHUB_URL="${1:-https://github.com/yukihamada/nanobot}"
REGION="ap-northeast-1"
BASE_DOMAIN="chatweb.ai"
HOSTED_ZONE_ID=$(aws route53 list-hosted-zones-by-name --query "HostedZones[?Name=='${BASE_DOMAIN}.'].Id" --output text | cut -d'/' -f3)

# ランダムなサブドメイン生成（8文字の英数字）
SUBDOMAIN=$(head /dev/urandom | LC_ALL=C tr -dc 'a-z0-9' | head -c 8)
FULL_DOMAIN="${SUBDOMAIN}.${BASE_DOMAIN}"
FUNCTION_NAME="nanobot-${SUBDOMAIN}"

echo "🚀 自動デプロイ＆サブドメイン割り当て"
echo "====================================="
echo "GitHub: $GITHUB_URL"
echo "サブドメイン: $FULL_DOMAIN"
echo "Lambda関数: $FUNCTION_NAME"
echo ""

# Step 1: GitHubからクローン
WORK_DIR="/tmp/nanobot-deploy-$$"
mkdir -p "$WORK_DIR"
cd "$WORK_DIR"

echo "📦 Step 1: GitHubからクローン..."
git clone --depth 1 "$GITHUB_URL" repo
cd repo

# Step 2: ARM64向けにコンパイル
echo ""
echo "🔨 Step 2: Rustコンパイル (ARM64)..."

if ! rustup target list | grep -q "aarch64-unknown-linux-musl (installed)"; then
    rustup target add aarch64-unknown-linux-musl
fi

# クロスコンパイル環境の確認
if ! command -v cross &> /dev/null; then
    echo "crossをインストール中..."
    cargo install cross
fi

# ビルド
cross build --release --target aarch64-unknown-linux-musl --bin chatweb

BINARY="target/aarch64-unknown-linux-musl/release/chatweb"
if [ ! -f "$BINARY" ]; then
    echo "❌ ビルド失敗: $BINARY が見つかりません"
    exit 1
fi

echo "✅ ビルド成功: $(du -h $BINARY | cut -f1)"

# Step 3: Lambda用にパッケージング
echo ""
echo "📦 Step 3: パッケージング..."

mkdir -p package
cp "$BINARY" package/bootstrap
chmod +x package/bootstrap

cd package
zip -q ../deployment.zip bootstrap
cd ..

PACKAGE_SIZE=$(du -h deployment.zip | cut -f1)
echo "✅ パッケージ作成: $PACKAGE_SIZE"

# Step 4: Lambda関数作成
echo ""
echo "🚀 Step 4: Lambda関数デプロイ..."

# IAMロール取得
ROLE_ARN=$(aws iam get-role --role-name nanobot-execution-role --query 'Role.Arn' --output text 2>/dev/null || \
    aws iam list-roles --query "Roles[?contains(RoleName, 'nanobot') && contains(RoleName, 'Role')].Arn | [0]" --output text)

if [ -z "$ROLE_ARN" ]; then
    echo "❌ IAMロールが見つかりません"
    exit 1
fi

# Lambda関数作成
aws lambda create-function \
    --function-name "$FUNCTION_NAME" \
    --runtime provided.al2023 \
    --role "$ROLE_ARN" \
    --handler bootstrap \
    --zip-file "fileb://deployment.zip" \
    --architectures arm64 \
    --timeout 120 \
    --memory-size 2048 \
    --region "$REGION" \
    --environment "Variables={RUST_LOG=info}" \
    --query 'FunctionArn' \
    --output text

echo "✅ Lambda関数作成完了"

# Lambda関数が利用可能になるまで待機
sleep 5

# Step 5: API Gateway設定
echo ""
echo "🌐 Step 5: API Gateway設定..."

# HTTP API作成
API_ID=$(aws apigatewayv2 create-api \
    --name "$FUNCTION_NAME" \
    --protocol-type HTTP \
    --target "arn:aws:lambda:${REGION}:$(aws sts get-caller-identity --query Account --output text):function:${FUNCTION_NAME}" \
    --region "$REGION" \
    --query 'ApiId' \
    --output text)

echo "✅ API Gateway作成: $API_ID"

# Lambda invoke権限を追加
aws lambda add-permission \
    --function-name "$FUNCTION_NAME" \
    --statement-id apigateway-invoke \
    --action lambda:InvokeFunction \
    --principal apigateway.amazonaws.com \
    --source-arn "arn:aws:execute-api:${REGION}:$(aws sts get-caller-identity --query Account --output text):${API_ID}/*" \
    --region "$REGION" > /dev/null

# デフォルトエンドポイント取得
API_ENDPOINT=$(aws apigatewayv2 get-api --api-id "$API_ID" --region "$REGION" --query 'ApiEndpoint' --output text)

echo "✅ APIエンドポイント: $API_ENDPOINT"

# Step 6: カスタムドメイン設定（ACM証明書が必要）
echo ""
echo "🔒 Step 6: カスタムドメイン設定..."

# ワイルドカード証明書を探す
CERT_ARN=$(aws acm list-certificates --region us-east-1 --query "CertificateSummaryList[?DomainName=='*.${BASE_DOMAIN}'].CertificateArn | [0]" --output text)

if [ "$CERT_ARN" = "None" ] || [ -z "$CERT_ARN" ]; then
    echo "⚠️  ワイルドカード証明書(*.${BASE_DOMAIN})が見つかりません"
    echo "   カスタムドメインはスキップします"
    CUSTOM_DOMAIN_CONFIGURED=false
else
    echo "✅ 証明書発見: $CERT_ARN"

    # カスタムドメイン作成
    DOMAIN_CONFIG=$(aws apigatewayv2 create-domain-name \
        --domain-name "$FULL_DOMAIN" \
        --domain-name-configurations CertificateArn="$CERT_ARN" \
        --region "$REGION" 2>&1)

    if echo "$DOMAIN_CONFIG" | grep -q "ConflictException"; then
        echo "⚠️  ドメイン ${FULL_DOMAIN} は既に存在します。別のサブドメインを生成中..."
        # 再試行（タイムスタンプ追加）
        SUBDOMAIN="${SUBDOMAIN}-$(date +%s)"
        FULL_DOMAIN="${SUBDOMAIN}.${BASE_DOMAIN}"
        echo "   新しいサブドメイン: $FULL_DOMAIN"

        DOMAIN_CONFIG=$(aws apigatewayv2 create-domain-name \
            --domain-name "$FULL_DOMAIN" \
            --domain-name-configurations CertificateArn="$CERT_ARN" \
            --region "$REGION")
    fi

    # API Gatewayとカスタムドメインをマッピング
    aws apigatewayv2 create-api-mapping \
        --domain-name "$FULL_DOMAIN" \
        --api-id "$API_ID" \
        --stage '$default' \
        --region "$REGION" > /dev/null

    # CloudFrontディストリビューションドメイン取得
    TARGET_DOMAIN=$(echo "$DOMAIN_CONFIG" | jq -r '.DomainNameConfigurations[0].ApiGatewayDomainName')

    # Route53レコード追加
    if [ -n "$HOSTED_ZONE_ID" ]; then
        cat > /tmp/route53-change.json << EOF
{
  "Changes": [{
    "Action": "CREATE",
    "ResourceRecordSet": {
      "Name": "${FULL_DOMAIN}",
      "Type": "CNAME",
      "TTL": 300,
      "ResourceRecords": [{"Value": "${TARGET_DOMAIN}"}]
    }
  }]
}
EOF

        aws route53 change-resource-record-sets \
            --hosted-zone-id "$HOSTED_ZONE_ID" \
            --change-batch file:///tmp/route53-change.json > /dev/null

        echo "✅ Route53レコード追加完了"
        CUSTOM_DOMAIN_CONFIGURED=true
    else
        echo "⚠️  Hosted Zoneが見つかりません"
        CUSTOM_DOMAIN_CONFIGURED=false
    fi
fi

# クリーンアップ
cd /
rm -rf "$WORK_DIR"

# 結果表示
echo ""
echo "================================"
echo "✅ デプロイ完了！"
echo "================================"
echo ""
echo "📊 詳細:"
echo "  Lambda関数:    $FUNCTION_NAME"
echo "  リージョン:    $REGION"
echo "  パッケージ:    $PACKAGE_SIZE"
echo ""
echo "🌐 アクセスURL:"
if [ "$CUSTOM_DOMAIN_CONFIGURED" = true ]; then
    echo "  https://${FULL_DOMAIN}/"
    echo "  (DNS伝播まで5-10分かかる場合があります)"
else
    echo "  ${API_ENDPOINT}"
fi
echo ""
echo "🧪 テスト:"
if [ "$CUSTOM_DOMAIN_CONFIGURED" = true ]; then
    echo "  curl https://${FULL_DOMAIN}/health"
else
    echo "  curl ${API_ENDPOINT}/health"
fi
echo ""
echo "🗑️  削除方法:"
echo "  aws lambda delete-function --function-name $FUNCTION_NAME --region $REGION"
echo "  aws apigatewayv2 delete-api --api-id $API_ID --region $REGION"
if [ "$CUSTOM_DOMAIN_CONFIGURED" = true ]; then
    echo "  aws apigatewayv2 delete-domain-name --domain-name $FULL_DOMAIN --region $REGION"
    echo "  # Route53レコードも手動で削除してください"
fi
