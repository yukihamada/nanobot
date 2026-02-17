#!/bin/bash
set -euo pipefail

# Lambda Runtime API
export AWS_LAMBDA_RUNTIME_API="${AWS_LAMBDA_RUNTIME_API}"

# Rustコンパイラの初期化
source $HOME/.cargo/env

while true; do
  # イベントを取得
  HEADERS="$(mktemp)"
  EVENT_DATA=$(curl -sS -LD "$HEADERS" -X GET "http://${AWS_LAMBDA_RUNTIME_API}/2018-06-01/runtime/invocation/next")
  REQUEST_ID=$(grep -Fi Lambda-Runtime-Aws-Request-Id "$HEADERS" | tr -d '[:space:]' | cut -d: -f2)

  # イベントからRustコードを取得
  RUST_CODE=$(echo "$EVENT_DATA" | jq -r '.code // ""')

  if [ -z "$RUST_CODE" ]; then
    # デフォルトの電卓コード
    RUST_CODE='fn main() { println!("123 + 456 = {}", 123 + 456); }'
  fi

  # /tmpにRustプロジェクトを作成
  cd /tmp
  rm -rf calculator
  cargo new calculator --bin 2>/dev/null || true
  cd calculator

  # main.rsを更新
  echo "$RUST_CODE" > src/main.rs

  # コンパイル
  COMPILE_OUTPUT=$(cargo build --release 2>&1 || echo "COMPILE_ERROR")

  if echo "$COMPILE_OUTPUT" | grep -q "COMPILE_ERROR"; then
    # コンパイルエラー
    RESPONSE="{\"statusCode\": 400, \"body\": $(echo "$COMPILE_OUTPUT" | jq -Rs .)}"
  else
    # 実行
    RUN_OUTPUT=$(./target/release/calculator 2>&1)
    RESPONSE="{\"statusCode\": 200, \"body\": $(echo "$RUN_OUTPUT" | jq -Rs .)}"
  fi

  # レスポンスを返す
  curl -X POST "http://${AWS_LAMBDA_RUNTIME_API}/2018-06-01/runtime/invocation/$REQUEST_ID/response" \
    -d "$RESPONSE"
done
