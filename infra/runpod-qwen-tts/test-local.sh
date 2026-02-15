#!/bin/bash
# Test Qwen3 Voice Cloning locally

set -e

echo "🧪 Testing Qwen3 Voice Cloning Service..."

# Check if server is running
if ! curl -s http://localhost:8000/health > /dev/null; then
    echo "❌ Server not running at http://localhost:8000"
    echo "Start it with: docker run -p 8000:8000 qwen3-voice-clone"
    exit 1
fi

echo "✅ Server is healthy"

# Test 1: Basic synthesis (Japanese)
echo "📝 Test 1: Japanese synthesis..."
curl -s -X POST "http://localhost:8000/synthesize" \
  -H "Content-Type: application/json" \
  -d '{
    "text": "こんにちは、世界！",
    "voice": "default",
    "language": "ja",
    "speed": 1.0
  }' | jq '.audio' -r | base64 -d > test_ja.wav

if [ -f test_ja.wav ] && [ -s test_ja.wav ]; then
    echo "✅ Japanese synthesis successful (test_ja.wav)"
else
    echo "❌ Japanese synthesis failed"
    exit 1
fi

# Test 2: English synthesis
echo "📝 Test 2: English synthesis..."
curl -s -X POST "http://localhost:8000/synthesize" \
  -H "Content-Type: application/json" \
  -d '{
    "text": "Hello, this is a test of voice synthesis!",
    "voice": "default",
    "language": "en",
    "speed": 1.0
  }' | jq '.audio' -r | base64 -d > test_en.wav

if [ -f test_en.wav ] && [ -s test_en.wav ]; then
    echo "✅ English synthesis successful (test_en.wav)"
else
    echo "❌ English synthesis failed"
    exit 1
fi

# Test 3: Speed variation
echo "📝 Test 3: Speed variation (1.5x)..."
curl -s -X POST "http://localhost:8000/synthesize" \
  -H "Content-Type: application/json" \
  -d '{
    "text": "これは速度テストです。",
    "voice": "default",
    "language": "ja",
    "speed": 1.5
  }' | jq '.audio' -r | base64 -d > test_speed.wav

if [ -f test_speed.wav ] && [ -s test_speed.wav ]; then
    echo "✅ Speed variation successful (test_speed.wav)"
else
    echo "❌ Speed variation failed"
    exit 1
fi

echo ""
echo "🎉 All tests passed!"
echo ""
echo "Generated files:"
ls -lh test_*.wav
echo ""
echo "Play audio with: afplay test_ja.wav  (macOS)"
echo "              or: aplay test_ja.wav   (Linux)"
