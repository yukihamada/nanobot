#!/usr/bin/env bash
# Landing page quality tests for chatweb.ai
# Run: bash tests/test_landing_page.sh [local|prod]
# Tests verify the HTML contains required elements for SEO, accessibility, and UX.

set -euo pipefail

MODE="${1:-local}"
if [ "$MODE" = "prod" ]; then
  URL="https://chatweb.ai"
else
  URL="file:///Users/yuki/workspace/nanobot/web/index.html"
fi

PASS=0
FAIL=0
ERRORS=""

check() {
  local desc="$1"
  local pattern="$2"
  local file="$3"
  if grep -q "$pattern" "$file"; then
    PASS=$((PASS + 1))
    echo "  PASS: $desc"
  else
    FAIL=$((FAIL + 1))
    ERRORS+="  FAIL: $desc (pattern: $pattern)\n"
    echo "  FAIL: $desc"
  fi
}

check_count() {
  local desc="$1"
  local pattern="$2"
  local min="$3"
  local file="$4"
  local count
  count=$(grep -c "$pattern" "$file" 2>/dev/null || echo 0)
  if [ "$count" -ge "$min" ]; then
    PASS=$((PASS + 1))
    echo "  PASS: $desc (found $count, min $min)"
  else
    FAIL=$((FAIL + 1))
    ERRORS+="  FAIL: $desc (found $count, need >= $min)\n"
    echo "  FAIL: $desc (found $count, need >= $min)"
  fi
}

echo "=== chatweb.ai Landing Page Tests ==="
echo "Mode: $MODE"
echo ""

# Fetch HTML
TMPFILE=$(mktemp)
if [ "$MODE" = "prod" ]; then
  curl -s "$URL" > "$TMPFILE"
else
  cp /Users/yuki/workspace/nanobot/web/index.html "$TMPFILE"
fi

echo "--- SEO & Meta Tags ---"
check "Has <title> tag" "<title>" "$TMPFILE"
check "Has meta description" 'meta name="description"' "$TMPFILE"
check "Has og:title" 'og:title' "$TMPFILE"
check "Has og:description" 'og:description' "$TMPFILE"
check "Has og:image" 'og:image' "$TMPFILE"
check "Has og:url" 'og:url' "$TMPFILE"
check "Has twitter:card" 'twitter:card' "$TMPFILE"
check "Has canonical URL" 'rel="canonical"' "$TMPFILE"
check "Has JSON-LD structured data" 'application/ld+json' "$TMPFILE"
check "Has lang attribute" 'lang="ja"' "$TMPFILE"

echo ""
echo "--- Accessibility & Semantics ---"
check "Has <h1> tag" "<h1" "$TMPFILE"
check_count "Has <h2> tags (sections)" "<h2" 2 "$TMPFILE"
check "Has <main> or role=main" "<main\|role=\"main\"" "$TMPFILE"
check "Has <nav> tag" "<nav" "$TMPFILE"
check "Has viewport meta" 'name="viewport"' "$TMPFILE"
check "Has theme-color meta" 'name="theme-color"' "$TMPFILE"
check "Has apple-mobile-web-app-capable" 'apple-mobile-web-app-capable' "$TMPFILE"

echo ""
echo "--- First Impression (What the site is) ---"
check "Has clear product name" 'chatweb.ai' "$TMPFILE"
check "Has tagline/subtitle visible" 'id="t-subtitle"' "$TMPFILE"
check "Has primary CTA button" 'id="t-hero-cta"' "$TMPFILE"
check "Has voice hero button" 'voice-hero-btn' "$TMPFILE"
check "Has suggestions for new users" 'suggestion' "$TMPFILE"

echo ""
echo "--- Trust & Social Proof ---"
check "Has stats section" 'stat-' "$TMPFILE"
check "Has trust badges (SSL/security)" 'trust-' "$TMPFILE"
check "Has pricing link" '/pricing' "$TMPFILE"

echo ""
echo "--- Core Functionality ---"
check "Has chat input" 'app-input' "$TMPFILE"
check "Has SSE streaming support" 'chat/stream' "$TMPFILE"
check "Has TTS support" 'playTTS\|speech/synthesize' "$TMPFILE"
check "Has voice input (STT)" 'webkitSpeechRecognition\|SpeechRecognition' "$TMPFILE"

echo ""
echo "--- i18n ---"
check "Has Japanese text" 'こんにちは\|何でも\|チャット' "$TMPFILE"
check "Has English fallback" "Hi!\|Ask me\|Try" "$TMPFILE"

echo ""
echo "==========================="
echo "Results: $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
  echo ""
  echo "Failures:"
  echo -e "$ERRORS"
  exit 1
else
  echo "All tests passed!"
  exit 0
fi
