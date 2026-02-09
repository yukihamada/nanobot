#!/usr/bin/env bash
# Sync API tests for ElioChat ↔ chatweb.ai conversation sync
# Run: bash tests/test_sync_api.sh [local|prod]
# Requires: a valid auth token (set CHATWEB_TOKEN env var)

set -euo pipefail

MODE="${1:-local}"
if [ "$MODE" = "prod" ]; then
  BASE="https://chatweb.ai"
else
  BASE="http://localhost:3000"
fi

TOKEN="${CHATWEB_TOKEN:-}"
PASS=0
FAIL=0
ERRORS=""

check_status() {
  local desc="$1"
  local expected="$2"
  local actual="$3"
  if [ "$actual" = "$expected" ]; then
    PASS=$((PASS + 1))
    echo "  PASS: $desc"
  else
    FAIL=$((FAIL + 1))
    ERRORS+="  FAIL: $desc (expected $expected, got $actual)\n"
    echo "  FAIL: $desc (expected $expected, got $actual)"
  fi
}

check_json() {
  local desc="$1"
  local jq_expr="$2"
  local expected="$3"
  local body="$4"
  local actual
  actual=$(echo "$body" | jq -r "$jq_expr" 2>/dev/null || echo "PARSE_ERROR")
  if [ "$actual" = "$expected" ]; then
    PASS=$((PASS + 1))
    echo "  PASS: $desc"
  else
    FAIL=$((FAIL + 1))
    ERRORS+="  FAIL: $desc (expected '$expected', got '$actual')\n"
    echo "  FAIL: $desc (expected '$expected', got '$actual')"
  fi
}

check_json_not_empty() {
  local desc="$1"
  local jq_expr="$2"
  local body="$3"
  local actual
  actual=$(echo "$body" | jq -r "$jq_expr" 2>/dev/null || echo "")
  if [ -n "$actual" ] && [ "$actual" != "null" ] && [ "$actual" != "" ]; then
    PASS=$((PASS + 1))
    echo "  PASS: $desc"
  else
    FAIL=$((FAIL + 1))
    ERRORS+="  FAIL: $desc (value was empty/null)\n"
    echo "  FAIL: $desc (value was empty/null)"
  fi
}

echo "=== Sync API Tests (${MODE}) ==="
echo "Base URL: $BASE"
echo ""

# ─── 1. Auth required (no token → 401 or error) ───
echo "--- 1. Authentication checks ---"

RESP=$(curl -s -w "\n%{http_code}" "$BASE/api/v1/sync/conversations" 2>/dev/null)
STATUS=$(echo "$RESP" | tail -1)
BODY=$(echo "$RESP" | sed '$d')
check_status "GET /sync/conversations without token returns 401" "401" "$STATUS"

RESP=$(curl -s -w "\n%{http_code}" "$BASE/api/v1/sync/conversations/test-id" 2>/dev/null)
STATUS=$(echo "$RESP" | tail -1)
check_status "GET /sync/conversations/{id} without token returns 401" "401" "$STATUS"

RESP=$(curl -s -w "\n%{http_code}" -X POST -H "Content-Type: application/json" \
  -d '{"conversations":[]}' "$BASE/api/v1/sync/push" 2>/dev/null)
STATUS=$(echo "$RESP" | tail -1)
check_status "POST /sync/push without token returns 401" "401" "$STATUS"

# ─── 2. With valid token ───
if [ -z "$TOKEN" ]; then
  echo ""
  echo "--- Skipping authenticated tests (set CHATWEB_TOKEN to run) ---"
else
  echo ""
  echo "--- 2. List conversations (authenticated) ---"

  RESP=$(curl -s -w "\n%{http_code}" \
    -H "Authorization: Bearer $TOKEN" \
    "$BASE/api/v1/sync/conversations" 2>/dev/null)
  STATUS=$(echo "$RESP" | tail -1)
  BODY=$(echo "$RESP" | sed '$d')
  check_status "GET /sync/conversations with token returns 200" "200" "$STATUS"
  check_json "Response has conversations array" ".conversations | type" "array" "$BODY"
  check_json_not_empty "Response has sync_token" ".sync_token" "$BODY"

  # ─── 3. With ?since filter ───
  echo ""
  echo "--- 3. List conversations with since filter ---"

  RESP=$(curl -s -w "\n%{http_code}" \
    -H "Authorization: Bearer $TOKEN" \
    "$BASE/api/v1/sync/conversations?since=2020-01-01T00:00:00Z" 2>/dev/null)
  STATUS=$(echo "$RESP" | tail -1)
  BODY=$(echo "$RESP" | sed '$d')
  check_status "GET /sync/conversations?since returns 200" "200" "$STATUS"
  check_json "Filtered response has conversations array" ".conversations | type" "array" "$BODY"

  # ─── 4. Push conversations ───
  echo ""
  echo "--- 4. Push conversations ---"

  CLIENT_ID="test-$(date +%s)"
  PUSH_BODY=$(cat <<ENDJSON
{
  "conversations": [
    {
      "client_id": "$CLIENT_ID",
      "title": "Test sync conversation",
      "messages": [
        {"role": "user", "content": "Hello from ElioChat sync test", "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"},
        {"role": "assistant", "content": "Hello! This is a synced response.", "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"}
      ]
    }
  ]
}
ENDJSON
)

  RESP=$(curl -s -w "\n%{http_code}" -X POST \
    -H "Authorization: Bearer $TOKEN" \
    -H "Content-Type: application/json" \
    -d "$PUSH_BODY" \
    "$BASE/api/v1/sync/push" 2>/dev/null)
  STATUS=$(echo "$RESP" | tail -1)
  BODY=$(echo "$RESP" | sed '$d')
  check_status "POST /sync/push returns 200" "200" "$STATUS"
  check_json "Response has synced array" ".synced | type" "array" "$BODY"
  check_json "Synced item has client_id" ".synced[0].client_id" "$CLIENT_ID" "$BODY"
  check_json_not_empty "Synced item has server_id" ".synced[0].server_id" "$BODY"

  # Extract server_id for next test
  SERVER_ID=$(echo "$BODY" | jq -r '.synced[0].server_id' 2>/dev/null || echo "")

  # ─── 5. Get pushed conversation ───
  echo ""
  echo "--- 5. Get pushed conversation messages ---"

  if [ -n "$SERVER_ID" ] && [ "$SERVER_ID" != "null" ]; then
    RESP=$(curl -s -w "\n%{http_code}" \
      -H "Authorization: Bearer $TOKEN" \
      "$BASE/api/v1/sync/conversations/$SERVER_ID" 2>/dev/null)
    STATUS=$(echo "$RESP" | tail -1)
    BODY=$(echo "$RESP" | sed '$d')
    check_status "GET /sync/conversations/{id} returns 200" "200" "$STATUS"
    check_json "Response has conversation_id" ".conversation_id" "$SERVER_ID" "$BODY"
    check_json_not_empty "Response has title" ".title" "$BODY"
    check_json "Response has messages array" ".messages | type" "array" "$BODY"
    check_json "First message is from user" ".messages[0].role" "user" "$BODY"
    check_json "First message content matches" ".messages[0].content" "Hello from ElioChat sync test" "$BODY"
  else
    echo "  SKIP: No server_id from push, skipping get test"
  fi

  # ─── 6. Verify pushed conversation appears in list ───
  echo ""
  echo "--- 6. Verify pushed conversation in list ---"

  RESP=$(curl -s -w "\n%{http_code}" \
    -H "Authorization: Bearer $TOKEN" \
    "$BASE/api/v1/sync/conversations" 2>/dev/null)
  STATUS=$(echo "$RESP" | tail -1)
  BODY=$(echo "$RESP" | sed '$d')
  check_status "GET /sync/conversations returns 200 after push" "200" "$STATUS"

  if [ -n "$SERVER_ID" ] && [ "$SERVER_ID" != "null" ]; then
    FOUND=$(echo "$BODY" | jq -r ".conversations[] | select(.id == \"$SERVER_ID\") | .id" 2>/dev/null || echo "")
    if [ "$FOUND" = "$SERVER_ID" ]; then
      PASS=$((PASS + 1))
      echo "  PASS: Pushed conversation appears in list"
    else
      FAIL=$((FAIL + 1))
      ERRORS+="  FAIL: Pushed conversation not found in list\n"
      echo "  FAIL: Pushed conversation not found in list"
    fi
  fi
fi

# ─── Summary ───
echo ""
echo "=== Results ==="
echo "PASS: $PASS"
echo "FAIL: $FAIL"
if [ $FAIL -gt 0 ]; then
  echo ""
  echo "Failures:"
  echo -e "$ERRORS"
  exit 1
fi
echo "All tests passed!"
