#!/bin/bash
# Deploy Qwen3 Voice Cloning to RunPod

set -e

echo "🚀 Deploying Qwen3 Voice Cloning to RunPod..."

# Configuration
IMAGE_NAME="qwen3-voice-clone"
REGISTRY="registry.runpod.io"
RUNPOD_API_KEY="${RUNPOD_API_KEY}"

if [ -z "$RUNPOD_API_KEY" ]; then
    echo "❌ Error: RUNPOD_API_KEY environment variable not set"
    echo "Please set it with: export RUNPOD_API_KEY=your_key_here"
    exit 1
fi

# Build Docker image
echo "📦 Building Docker image..."
docker build -t ${IMAGE_NAME}:latest .

# Tag for RunPod registry
FULL_IMAGE="${REGISTRY}/${IMAGE_NAME}:latest"
docker tag ${IMAGE_NAME}:latest ${FULL_IMAGE}

# Push to registry
echo "⬆️  Pushing to RunPod registry..."
docker push ${FULL_IMAGE}

# Deploy via RunPod API
echo "🎯 Deploying to RunPod..."

RESPONSE=$(curl -s -X POST "https://api.runpod.io/graphql" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${RUNPOD_API_KEY}" \
  -d '{
    "query": "mutation { podCreate(input: { name: \"qwen3-voice-clone\", imageName: \"'${FULL_IMAGE}'\", gpuTypeId: \"NVIDIA RTX A4000\", cloudType: SECURE, volumeInGb: 20, containerDiskInGb: 20, minVcpuCount: 4, minMemoryInGb: 16, env: [{key: \"MODEL_NAME\", value: \"Qwen/Qwen2.5-TTS\"}], ports: \"8000/http\" }) { id status } }"
  }')

POD_ID=$(echo $RESPONSE | jq -r '.data.podCreate.id')

if [ "$POD_ID" != "null" ]; then
    echo "✅ Deployment successful!"
    echo "Pod ID: $POD_ID"
    echo ""
    echo "📝 Next steps:"
    echo "1. Wait for pod to be ready (check at https://runpod.io)"
    echo "2. Get the pod URL from RunPod dashboard"
    echo "3. Set RUNPOD_QWEN_TTS_URL in your environment"
    echo "   export RUNPOD_QWEN_TTS_URL=https://your-pod-id-8000.proxy.runpod.net"
else
    echo "❌ Deployment failed!"
    echo "Response: $RESPONSE"
    exit 1
fi
