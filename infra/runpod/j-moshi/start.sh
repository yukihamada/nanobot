#!/bin/bash
# J-Moshi startup script for RunPod
set -e

echo "=== J-Moshi Server ==="
echo "GPU: $(nvidia-smi --query-gpu=name,memory.total --format=csv,noheader)"

# Use Gradio tunnel for public HTTPS access (required for microphone/getUserMedia)
exec python3 -m moshi.server \
    --hf-repo nu-dialogue/j-moshi \
    --gradio-tunnel \
    --device cuda \
    --host 0.0.0.0 \
    --port 8998
