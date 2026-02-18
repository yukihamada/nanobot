#!/bin/bash
set -e

# nanobotにRustコードを生成させて、Lambda上でコンパイル＆デプロイ
# 使い方: ./compile-and-deploy.sh "電卓を作って"

PROMPT="${1:-電卓プログラムを作って}"
REGION="ap-northeast-1"

echo "🤖 Step 1: nanobotにRustコードを生成させる"
echo "============================================"

# nanobotにコード生成を依頼
RUST_CODE=$(nanobot agent -m "Create a simple Rust program: $PROMPT

Requirements:
- Pure Rust (no external dependencies if possible)
- Simple, working code
- Output to stdout

Just give me the Rust code for src/main.rs, nothing else." | tail -n +10)

echo "Generated code:"
echo "$RUST_CODE"
echo ""

echo "🔨 Step 2: Lambdaでコンパイル"
echo "============================"

# Lambdaを呼び出してコンパイル
aws lambda invoke \
  --function-name rust-compiler-lambda \
  --payload "{\"code\":$(echo "$RUST_CODE" | jq -Rs .)}" \
  --region $REGION \
  response.json

echo ""
echo "✅ Result:"
cat response.json | jq .
