# CosyVoice 2 TTS for RunPod
FROM nvidia/cuda:12.1.0-runtime-ubuntu22.04

# Install system dependencies
RUN apt-get update && apt-get install -y \
    python3.11 \
    python3-pip \
    git \
    ffmpeg \
    libsndfile1 \
    sox \
    && rm -rf /var/lib/apt/lists/*

# Set working directory
WORKDIR /app

# Install Python packages
RUN pip3 install --no-cache-dir \
    runpod \
    torch \
    torchaudio \
    soundfile \
    numpy \
    setuptools \
    requests

# Clone CosyVoice repository
RUN cd /app && \
    git clone --depth=1 https://github.com/FunAudioLLM/CosyVoice.git && \
    cd CosyVoice && \
    pip3 install --no-deps -r requirements.txt || true

# Download CosyVoice2 model
RUN python3 -c "from modelscope import snapshot_download; \
    snapshot_download('iic/CosyVoice2-0.5B', cache_dir='/root/.cache/modelscope'); \
    print('Model cached')"

# Copy handler
COPY handler.py /app/

# Set Python path
ENV PYTHONPATH="/app/CosyVoice:/app/CosyVoice/third_party/Matcha-TTS:$PYTHONPATH"
ENV MODEL_DIR="/root/.cache/modelscope/iic/CosyVoice2-0___5B"

# RunPod handler
CMD ["python3", "-u", "handler.py"]
