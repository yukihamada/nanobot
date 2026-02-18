#!/bin/bash
# Deploy J-Moshi to RunPod GPU Pod
# Usage: ./deploy.sh [quick|docker]
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

RUNPOD_API_KEY="${RUNPOD_API_KEY:?Set RUNPOD_API_KEY}"

# --- Config ---
POD_NAME="j-moshi"
GPU_TYPE="NVIDIA GeForce RTX 4090"   # 24GB VRAM, ~$0.34/hr community
IMAGE="runpod/pytorch:2.4.0-py3.11-cuda12.4.1-devel-ubuntu22.04"
CONTAINER_DISK=50
CLOUD_TYPE="COMMUNITY"

START_CMD="bash -c 'apt-get update && apt-get install -y libopus-dev ffmpeg && pip install moshi==0.2.2 gradio huggingface-hub sentencepiece && python3 -m moshi.server --hf-repo nu-dialogue/j-moshi --gradio-tunnel --device cuda --host 0.0.0.0 --port 8998'"

# --- Deploy via RunPod Python SDK ---
deploy() {
    local image="${1:-$IMAGE}"
    local args="${2:-$START_CMD}"

    python3 -c "
import runpod
runpod.api_key = '${RUNPOD_API_KEY}'

pod = runpod.create_pod(
    name='${POD_NAME}',
    image_name='${image}',
    gpu_type_id='${GPU_TYPE}',
    cloud_type='${CLOUD_TYPE}',
    container_disk_in_gb=${CONTAINER_DISK},
    volume_in_gb=0,
    min_vcpu_count=4,
    min_memory_in_gb=16,
    docker_args=\"\"\"${args}\"\"\",
    ports='8998/http',
)
pod_id = pod['id']
print(f'Pod created: {pod_id}')
print(f'Dashboard: https://www.runpod.io/console/pods/{pod_id}')
print(f'Proxy URL: https://{pod_id}-8998.proxy.runpod.net')
print()
print('Gradio tunnel URL will appear in pod logs once model loads.')
print('Check logs: RunPod Dashboard > Pods > j-moshi > Logs')
"
}

# --- Quick: use RunPod cached image, install at startup ---
quick_deploy() {
    echo "Deploying J-Moshi (quick mode)..."
    deploy "$IMAGE" "$START_CMD"
}

# --- Docker: pre-bake model into image ---
docker_deploy() {
    echo "Building J-Moshi Docker image..."
    docker build -t j-moshi:latest .

    DOCKER_USER="${DOCKER_USER:?Set DOCKER_USER for Docker Hub push}"
    FULL_IMAGE="${DOCKER_USER}/j-moshi:latest"

    echo "Pushing ${FULL_IMAGE}..."
    docker tag j-moshi:latest "$FULL_IMAGE"
    docker push "$FULL_IMAGE"

    echo "Creating RunPod pod..."
    deploy "$FULL_IMAGE" ""
}

case "${1:-quick}" in
    quick)  quick_deploy ;;
    docker) docker_deploy ;;
    *)
        echo "Usage: $0 [quick|docker]"
        echo "  quick  — RunPod cached image, install at startup (~10 min)"
        echo "  docker — Pre-baked image, faster starts (~2 min)"
        ;;
esac
