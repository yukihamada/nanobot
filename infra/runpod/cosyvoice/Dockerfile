FROM nvidia/cuda:12.1.0-cudnn8-runtime-ubuntu22.04

ENV DEBIAN_FRONTEND=noninteractive
ENV PYTHONUNBUFFERED=1

RUN apt-get update && apt-get install -y --no-install-recommends \
    python3.10 python3-pip python3-dev git sox libsox-dev \
    ffmpeg wget ca-certificates && \
    apt-get clean && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Clone CosyVoice repo
RUN git clone --recursive https://github.com/FunAudioLLM/CosyVoice.git /app && \
    cd /app && git submodule update --init --recursive

# Install dependencies
RUN pip3 install --no-cache-dir -r requirements.txt && \
    pip3 install --no-cache-dir runpod requests

# Download CosyVoice2-0.5B model
RUN python3 -c "from huggingface_hub import snapshot_download; \
    snapshot_download('FunAudioLLM/CosyVoice2-0.5B', local_dir='pretrained_models/CosyVoice2-0.5B')"

# Copy handler
COPY handler.py /app/handler.py

# Create speakers directory for custom voice embeddings
RUN mkdir -p /data/speakers

ENV MODEL_DIR=pretrained_models/CosyVoice2-0.5B
ENV SPEAKERS_DIR=/data/speakers

CMD ["python3", "-u", "/app/handler.py"]
