# Qwen3 Voice Cloning TTS for RunPod
FROM nvidia/cuda:12.1.0-runtime-ubuntu22.04

# Install system dependencies
RUN apt-get update && apt-get install -y \
    python3.11 \
    python3-pip \
    ffmpeg \
    libsndfile1 \
    && rm -rf /var/lib/apt/lists/*

# Set working directory
WORKDIR /app

# Install Python packages
RUN pip3 install --no-cache-dir \
    runpod \
    torch \
    torchaudio \
    transformers>=4.40.0 \
    soundfile \
    numpy \
    pydub \
    fastapi \
    uvicorn

# Copy handler
COPY handler.py /app/

# Pre-download Qwen3-TTS model
RUN python3 -c "from transformers import AutoModel, AutoProcessor; \
    model = AutoModel.from_pretrained('Qwen/Qwen-Audio-Chat', trust_remote_code=True); \
    processor = AutoProcessor.from_pretrained('Qwen/Qwen-Audio-Chat', trust_remote_code=True); \
    print('Model cached')"

# RunPod handler
CMD ["python3", "-u", "handler.py"]
