# RunPod Qwen3 Voice Cloning Dockerfile
FROM pytorch/pytorch:2.1.0-cuda12.1-cudnn8-runtime

# Install system dependencies
RUN apt-get update && apt-get install -y \
    git \
    ffmpeg \
    libsndfile1 \
    && rm -rf /var/lib/apt/lists/*

# Set working directory
WORKDIR /app

# Install Python dependencies
RUN pip install --no-cache-dir \
    fastapi==0.109.0 \
    uvicorn[standard]==0.27.0 \
    transformers==4.40.0 \
    accelerate==0.27.0 \
    soundfile==0.12.1 \
    librosa==0.10.1 \
    scipy==1.12.0 \
    pydantic==2.6.0 \
    python-multipart==0.0.9 \
    torchaudio==2.1.0

# Copy application code
COPY server.py /app/
COPY requirements.txt /app/

# Expose port
EXPOSE 8000

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=60s --retries=3 \
    CMD curl -f http://localhost:8000/health || exit 1

# Run the server
CMD ["uvicorn", "server:app", "--host", "0.0.0.0", "--port", "8000"]
