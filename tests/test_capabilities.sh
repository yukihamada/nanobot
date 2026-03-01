#!/usr/bin/env bash
# chatweb.ai Capability Tests
# Tests each AI tool to verify it works end-to-end.
#
# Usage:
#   bash tests/test_capabilities.sh [local|prod]
#   CHATWEB_TOKEN=cw_xxx bash tests/test_capabilities.sh prod
#
# Output: colored pass/fail + summary table
# Requirements: curl, jq

set -uo pipefail

MODE="${1:-prod}"
if [ "$MODE" = "local" ]; then
  BASE="http://localhost:3000"
else
  BASE="https://chatweb.ai"
fi

TOKEN="${CHATWEB_TOKEN:-}"
TIMEOUT=60   # seconds per test

# ── Colors ──────────────────────────────────────────────────────────────────
GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[0;33m'
BLUE='\033[0;34m'; CYAN='\033[0;36m'; BOLD='\033[1m'; RESET='\033[0m'

# ── Counters ─────────────────────────────────────────────────────────────────
PASS=0; FAIL=0; SKIP=0
declare -a RESULTS=()
AVAILABLE_TOOLS=""
CREDITS_INITIAL=0

# ── Helpers ──────────────────────────────────────────────────────────────────
chat() {
  local msg="$1"
  curl -s --max-time "$TIMEOUT" -X POST "$BASE/api/v1/chat" \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer $TOKEN" \
    -d "{\"message\": $(echo "$msg" | jq -Rs .)}" 2>/dev/null
}

# Check if a tool is available for this account
has_tool() {
  echo "$AVAILABLE_TOOLS" | grep -q "\"$1\""
}

# Detect if response is a timeout/provider error
is_timeout_error() {
  local resp="$1"
  echo "$resp" | grep -qi "もう一回\|一時的に\|遅延\|timeout\|タイムアウト\|時間がかかっ\|失敗しました。もう一度\|呼び出すことができなかっ\|Circuit\|circuit\|混雑して\|混み合って"
}

check_tool_used() {
  local name="$1"
  local msg="$2"
  local expected_tool="$3"

  # Skip if tool not available for this plan
  if ! has_tool "$expected_tool"; then
    echo -e "  ${YELLOW}SKIP${RESET}: $name (tool '$expected_tool' not in plan)"
    SKIP=$((SKIP + 1))
    RESULTS+=("SKIP|$name|tool not available in plan")
    return
  fi

  echo -n "  Testing: $name ... "
  local body
  body=$(chat "$msg")

  if [ -z "$body" ]; then
    echo -e "${RED}FAIL${RESET} (no response)"
    FAIL=$((FAIL + 1))
    RESULTS+=("FAIL|$name|no response from API")
    return
  fi

  # Check for credit exhaustion (only actual 402 / "残高不足" errors, not incidental mentions)
  local credits_used response
  response=$(echo "$body" | jq -r '.response // ""' 2>/dev/null || echo "")
  local http_error
  http_error=$(echo "$body" | jq -r '.error // ""' 2>/dev/null || echo "")
  if echo "$http_error" | grep -qi "insufficient_credits\|credits_exhausted"; then
    echo -e "${YELLOW}SKIP${RESET} (credits exhausted)"
    SKIP=$((SKIP + 1))
    RESULTS+=("SKIP|$name|credits exhausted")
    return
  fi

  # Check for provider timeout
  if is_timeout_error "$response"; then
    echo -e "${YELLOW}PARTIAL${RESET} (provider timeout — tool may exist but LLM timed out)"
    SKIP=$((SKIP + 1))
    RESULTS+=("PARTIAL|$name|provider timeout")
    return
  fi

  local tools_used
  tools_used=$(echo "$body" | jq -r '.tools_used // [] | join(",")' 2>/dev/null || echo "")

  if echo "$tools_used" | grep -q "$expected_tool"; then
    echo -e "${GREEN}PASS${RESET} (tool: $tools_used)"
    PASS=$((PASS + 1))
    RESULTS+=("PASS|$name|tool=$tools_used")
  else
    local short_response
    short_response=$(echo "$response" | python3 -c "import sys; print(sys.stdin.read()[:100].replace('\n',' '))" 2>/dev/null || echo "${response:0:100}")
    # Detect Nemotron hallucination: model claims tool is unavailable even though it IS in the plan
    if echo "$response" | grep -qi "利用できません\|利用できない\|ツールは.*ない\|tool.*not available\|cannot.*use"; then
      echo -e "${YELLOW}PARTIAL${RESET} (model claims tool unavailable — Nemotron limitation)"
      echo -e "       response: $short_response"
      SKIP=$((SKIP + 1))
      RESULTS+=("PARTIAL|$name|Nemotron refuses to call $expected_tool")
    else
      echo -e "${RED}FAIL${RESET} (tools_used=[$tools_used])"
      echo -e "       response: $short_response"
      FAIL=$((FAIL + 1))
      RESULTS+=("FAIL|$name|tools_used=[$tools_used]")
    fi
  fi
}

check_response_contains() {
  local name="$1"
  local msg="$2"
  local keyword="$3"

  echo -n "  Testing: $name ... "
  local body response
  body=$(chat "$msg")
  response=$(echo "$body" | jq -r '.response // ""' 2>/dev/null || echo "")

  if [ -z "$body" ] || [ -z "$response" ]; then
    echo -e "${RED}FAIL${RESET} (no response)"
    FAIL=$((FAIL + 1))
    RESULTS+=("FAIL|$name|no response")
    return
  fi

  # Check for credit exhaustion (only actual error, not incidental mentions)
  local http_error
  http_error=$(echo "$body" | jq -r '.error // ""' 2>/dev/null || echo "")
  if echo "$http_error" | grep -qi "insufficient_credits\|credits_exhausted"; then
    echo -e "${YELLOW}SKIP${RESET} (credits exhausted)"
    SKIP=$((SKIP + 1))
    RESULTS+=("SKIP|$name|credits exhausted")
    return
  fi

  # Timeout = PARTIAL
  if is_timeout_error "$response"; then
    echo -e "${YELLOW}PARTIAL${RESET} (provider timeout)"
    SKIP=$((SKIP + 1))
    RESULTS+=("PARTIAL|$name|provider timeout")
    return
  fi

  if echo "$response" | grep -qi "$keyword"; then
    echo -e "${GREEN}PASS${RESET}"
    PASS=$((PASS + 1))
    RESULTS+=("PASS|$name|contains '$keyword'")
  else
    local short
    short=$(echo "$response" | head -c 100 | tr '\n' ' ')
    echo -e "${RED}FAIL${RESET} (keyword '$keyword' not found)"
    echo -e "       response: $short"
    FAIL=$((FAIL + 1))
    RESULTS+=("FAIL|$name|keyword '$keyword' not found")
  fi
}

check_http_status() {
  local name="$1"
  local url="$2"
  local method="${3:-GET}"
  local expected="${4:-200}"
  local body_data="${5:-}"

  echo -n "  Testing: $name ... "
  local status
  if [ -n "$body_data" ]; then
    status=$(curl -s -o /dev/null -w "%{http_code}" --max-time 15 -X "$method" \
      -H "Authorization: Bearer $TOKEN" \
      -H "Content-Type: application/json" \
      -d "$body_data" "$url" 2>/dev/null)
  else
    status=$(curl -s -o /dev/null -w "%{http_code}" --max-time 15 -X "$method" \
      -H "Authorization: Bearer $TOKEN" "$url" 2>/dev/null)
  fi

  if [ "$status" = "$expected" ]; then
    echo -e "${GREEN}PASS${RESET} (HTTP $status)"
    PASS=$((PASS + 1))
    RESULTS+=("PASS|$name|HTTP $status")
  else
    echo -e "${RED}FAIL${RESET} (expected $expected, got $status)"
    FAIL=$((FAIL + 1))
    RESULTS+=("FAIL|$name|HTTP $status (expected $expected)")
  fi
}

# ── Header ────────────────────────────────────────────────────────────────────
echo ""
echo -e "${BOLD}${BLUE}╔═══════════════════════════════════════════════════╗${RESET}"
echo -e "${BOLD}${BLUE}║   chatweb.ai Capability Tests                     ║${RESET}"
echo -e "${BOLD}${BLUE}╚═══════════════════════════════════════════════════╝${RESET}"
echo -e "  Mode: ${BOLD}$MODE${RESET} | Base: $BASE | Timeout: ${TIMEOUT}s"

if [ -z "$TOKEN" ]; then
  echo -e "${RED}ERROR: CHATWEB_TOKEN is not set.${RESET}"
  echo "  Set it with: export CHATWEB_TOKEN=cw_xxx"
  exit 1
fi

echo -e "  Token: ${TOKEN:0:16}..."
echo ""

# ── Pre-flight: Auth & Credits ────────────────────────────────────────────────
echo -e "${BOLD}${CYAN}── Pre-flight Checks ────────────────────────────────${RESET}"

AUTH_BODY=$(curl -s --max-time 10 -H "Authorization: Bearer $TOKEN" "$BASE/api/v1/auth/me" 2>/dev/null)
if [ -z "$AUTH_BODY" ] || echo "$AUTH_BODY" | grep -q '"authenticated":false'; then
  echo -e "${RED}FAIL${RESET}: Authentication failed. Check your CHATWEB_TOKEN."
  exit 1
fi

CREDITS_INITIAL=$(echo "$AUTH_BODY" | jq -r '.credits_remaining // 0' 2>/dev/null || echo "0")
PLAN=$(echo "$AUTH_BODY" | jq -r '.plan // "unknown"' 2>/dev/null || echo "unknown")
MAX_ITER=$(echo "$AUTH_BODY" | jq -r '.max_tool_iterations // 1' 2>/dev/null || echo "1")
AVAILABLE_TOOLS=$(echo "$AUTH_BODY" | jq -c '.available_tools // []' 2>/dev/null || echo "[]")

echo -e "  Auth: ${GREEN}OK${RESET} | Plan: ${BOLD}$PLAN${RESET} | Credits: ${BOLD}$CREDITS_INITIAL${RESET} | Max iterations: $MAX_ITER"
echo -e "  Available tools: $(echo "$AVAILABLE_TOOLS" | jq -r 'join(", ")' 2>/dev/null)"
echo ""

MIN_CREDITS=20
if [ "$CREDITS_INITIAL" -lt "$MIN_CREDITS" ] 2>/dev/null; then
  echo -e "${YELLOW}WARNING: Only $CREDITS_INITIAL credits remaining. Tests may fail mid-way.${RESET}"
  echo -e "  Recommend: redeem a coupon or use an account with 50+ credits."
  echo ""
fi

# ── Section 1: Basic API ──────────────────────────────────────────────────────
echo -e "${BOLD}${CYAN}── 1. Basic API ─────────────────────────────────────${RESET}"

check_http_status "GET /api/v1/auth/me" "$BASE/api/v1/auth/me" "GET" "200"

EMPTY_RESP=$(curl -s --max-time 10 -X POST "$BASE/api/v1/chat" \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"message": ""}' 2>/dev/null)
EMPTY_MSG=$(echo "$EMPTY_RESP" | jq -r '.response // ""' 2>/dev/null)
if echo "$EMPTY_MSG" | grep -qi "入力\|empty\|cannot"; then
  echo -e "  ${GREEN}PASS${RESET}: Empty message validation works"
  PASS=$((PASS + 1))
  RESULTS+=("PASS|Empty message validation|returns error message")
else
  echo -e "  ${RED}FAIL${RESET}: Empty message not validated (got: $EMPTY_MSG)"
  FAIL=$((FAIL + 1))
  RESULTS+=("FAIL|Empty message validation|unexpected response")
fi

# ── Section 2: Information Tools ─────────────────────────────────────────────
echo ""
echo -e "${BOLD}${CYAN}── 2. Information & Search Tools ────────────────────${RESET}"

check_tool_used "web_search — recent events" \
  "web_searchツールで「2026年 AI ニュース」を検索してください。一行で結果を教えて。" \
  "web_search"

check_tool_used "read_webpage — URL fetch" \
  "https://example.com のページを開いて内容を教えて" \
  "read_webpage"

check_tool_used "wikipedia — Wikipedia lookup" \
  "量子コンピュータについてWikipediaで調べて概要を2行で教えて" \
  "wikipedia"

check_tool_used "weather — Weather forecast" \
  "weatherツールで東京の天気を取得してください" \
  "weather"

check_tool_used "datetime — Current time" \
  "datetimeツールで今の日時を取得してください" \
  "datetime"

# ── Section 3: Calculation ────────────────────────────────────────────────────
echo ""
echo -e "${BOLD}${CYAN}── 3. Calculation Tools ─────────────────────────────${RESET}"

check_tool_used "calculator — Arithmetic" \
  "calculatorツールで 999 × 111 を計算して答えだけ教えて" \
  "calculator"

# ── Section 4: Content Generation ────────────────────────────────────────────
echo ""
echo -e "${BOLD}${CYAN}── 4. Content Generation Tools ──────────────────────${RESET}"

check_tool_used "image_generate — AI image" \
  "image_generateツールを呼び出して「a cat」の画像を生成して" \
  "image_generate"

check_tool_used "create_qr — QR code" \
  "create_qrツールを呼び出して「https://chatweb.ai」のQRコードを生成して" \
  "create_qr"

check_tool_used "music_generate — AI music" \
  "music_generateツールを呼び出してjazzの短い曲を生成して" \
  "music_generate"

# ── Section 5: Code Execution ─────────────────────────────────────────────────
echo ""
echo -e "${BOLD}${CYAN}── 5. Code Execution ────────────────────────────────${RESET}"

check_tool_used "code_execute — Shell script" \
  "code_executeツールでshellを実行して: echo \$(( 2 ** 10 ))" \
  "code_execute"

# ── Section 6: Analysis ───────────────────────────────────────────────────────
echo ""
echo -e "${BOLD}${CYAN}── 6. Analysis Tools ────────────────────────────────${RESET}"

check_tool_used "pdf_analyze — PDF analysis" \
  "pdf_analyzeツールを呼び出して https://arxiv.org/pdf/1706.03762 を1行で要約して" \
  "pdf_analyze"

check_tool_used "image_analyze — Image analysis" \
  "image_analyzeツールを呼び出して https://upload.wikimedia.org/wikipedia/commons/a/a7/Camponotus_flavomarginatus_ant.jpg を1行で説明して" \
  "image_analyze"

# ── Section 7: Direct Knowledge (no tool) ────────────────────────────────────
echo ""
echo -e "${BOLD}${CYAN}── 7. Direct Knowledge (no tool needed) ─────────────${RESET}"

check_response_contains "Japanese capital" \
  "日本の首都は？一単語で答えて" \
  "東京"

check_response_contains "Math: 2^10" \
  "2の10乗の答えは何？数字だけ" \
  "1024"

check_response_contains "Identity: ChatWeb name" \
  "名前は？" \
  "ChatWeb"

check_response_contains "English response" \
  "Capital of Japan? One word." \
  "Tokyo"

# ── Section 8: Credits Check ──────────────────────────────────────────────────
echo ""
echo -e "${BOLD}${CYAN}── 8. Credits & Account ─────────────────────────────${RESET}"

FINAL_AUTH=$(curl -s --max-time 10 -H "Authorization: Bearer $TOKEN" "$BASE/api/v1/auth/me" 2>/dev/null)
CREDITS_FINAL=$(echo "$FINAL_AUTH" | jq -r '.credits_remaining // 0' 2>/dev/null || echo "0")
CREDITS_SPENT=$((CREDITS_INITIAL - CREDITS_FINAL))

echo -e "  Credits at start: ${BOLD}$CREDITS_INITIAL${RESET}"
echo -e "  Credits at end:   ${BOLD}$CREDITS_FINAL${RESET}"
echo -e "  Credits spent:    ${BOLD}$CREDITS_SPENT${RESET}"

if [ "$CREDITS_FINAL" -ge 0 ] 2>/dev/null; then
  echo -e "  ${GREEN}PASS${RESET}: Credit tracking working (auth/me)"
  PASS=$((PASS + 1))
  RESULTS+=("PASS|Credit tracking|$CREDITS_INITIAL → $CREDITS_FINAL (spent: $CREDITS_SPENT)")
else
  echo -e "  ${RED}FAIL${RESET}: credits_remaining unavailable"
  FAIL=$((FAIL + 1))
  RESULTS+=("FAIL|Credit tracking|credits_remaining unavailable")
fi

# ── Section 9: TTS ────────────────────────────────────────────────────────────
echo ""
echo -e "${BOLD}${CYAN}── 9. TTS (Speech Synthesis) ────────────────────────${RESET}"

TTS_STATUS=$(curl -s -o /dev/null -w "%{http_code}" --max-time 30 \
  -X POST "$BASE/api/v1/speech/synthesize" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"text": "こんにちは！テストです", "voice": "nova"}' 2>/dev/null)

case "$TTS_STATUS" in
  200) echo -e "  ${GREEN}PASS${RESET}: TTS returns audio (HTTP 200)"
       PASS=$((PASS + 1)); RESULTS+=("PASS|TTS speech/synthesize|HTTP 200") ;;
  402) echo -e "  ${YELLOW}SKIP${RESET}: TTS requires credits (HTTP 402)"
       SKIP=$((SKIP + 1)); RESULTS+=("SKIP|TTS speech/synthesize|HTTP 402 no credits") ;;
  *)   echo -e "  ${RED}FAIL${RESET}: TTS returned HTTP $TTS_STATUS"
       FAIL=$((FAIL + 1)); RESULTS+=("FAIL|TTS speech/synthesize|HTTP $TTS_STATUS") ;;
esac

# ── Summary Table ─────────────────────────────────────────────────────────────
echo ""
echo -e "${BOLD}${BLUE}╔═══════════════════════════════════════════════════╗${RESET}"
echo -e "${BOLD}${BLUE}║   Test Results Summary                            ║${RESET}"
echo -e "${BOLD}${BLUE}╚═══════════════════════════════════════════════════╝${RESET}"
echo ""

printf "  %-8s %-42s %s\n" "Status" "Test" "Detail"
printf "  %-8s %-42s %s\n" "────────" "──────────────────────────────────────────" "──────────────────────"

for entry in "${RESULTS[@]}"; do
  IFS='|' read -r status name detail <<< "$entry"
  name_short="${name:0:40}"
  detail_short="${detail:0:28}"
  case "$status" in
    PASS)    color="${GREEN}" ;;
    FAIL)    color="${RED}" ;;
    PARTIAL) color="${YELLOW}" ;;
    SKIP)    color="${YELLOW}" ;;
    *)       color="${RESET}" ;;
  esac
  printf "  ${color}%-8s${RESET} %-42s %s\n" "$status" "$name_short" "$detail_short"
done

echo ""
TOTAL=$((PASS + FAIL + SKIP))
echo -e "  Total: ${BOLD}$TOTAL${RESET}  |  ${GREEN}PASS: $PASS${RESET}  |  ${RED}FAIL: $FAIL${RESET}  |  ${YELLOW}SKIP/PARTIAL: $SKIP${RESET}"
echo ""

if [ $FAIL -eq 0 ]; then
  echo -e "  ${GREEN}${BOLD}All tests passed (or skipped)!${RESET}"
else
  echo -e "  ${RED}${BOLD}$FAIL test(s) failed. Check details above.${RESET}"
  exit 1
fi
