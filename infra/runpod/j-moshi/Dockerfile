# J-Moshi: Japanese full-duplex spoken dialogue on RunPod
FROM pytorch/pytorch:2.4.0-py3.11-cuda12.4.1-devel-ubuntu22.04

RUN apt-get update && apt-get install -y \
    libopus-dev \
    ffmpeg \
    git \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

RUN pip install --no-cache-dir \
    "moshi<=0.2.2" \
    gradio \
    huggingface-hub \
    sentencepiece

# Pre-download model weights (~14GB) into image
RUN python3 -c "\
from huggingface_hub import snapshot_download; \
snapshot_download('nu-dialogue/j-moshi'); \
print('J-Moshi model cached')"

COPY start.sh /app/start.sh
RUN chmod +x /app/start.sh

EXPOSE 8998

CMD ["/app/start.sh"]
