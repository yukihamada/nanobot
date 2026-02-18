"""
J-Moshi on Modal — Japanese full-duplex spoken dialogue system.

nu-dialogue/j-moshi: Nagoya University's Japanese adaptation of Moshi (7B).
Real-time voice conversation with overlapping speech and backchannels (相槌).

Deploy:  modal deploy j_moshi.py
Run:     modal run j_moshi.py
Logs:    modal app logs j-moshi

Requires headphones (not speakers) to avoid echo feedback.
Cost: ~$1.10/hour (A10G) when running, $0 when idle.
License: CC BY-NC 4.0 (research use only).
"""

import modal
import subprocess
import sys

# ---------------------------------------------------------------------------
# Modal image: moshi + dependencies
# ---------------------------------------------------------------------------
moshi_image = (
    modal.Image.debian_slim(python_version="3.12")
    .apt_install("libopus-dev", "ffmpeg", "git")
    .pip_install(
        "moshi<=0.2.2",
        "gradio>=4.0",
        "huggingface-hub",
        "sentencepiece",
    )
    # Pre-download model weights during image build (avoids cold-start download)
    .run_commands(
        "python3 -c \""
        "from huggingface_hub import snapshot_download; "
        "snapshot_download('nu-dialogue/j-moshi'); "
        "print('J-Moshi model cached')"
        "\""
    )
)

app = modal.App("j-moshi", image=moshi_image)


# ---------------------------------------------------------------------------
# Server function — launches moshi.server with Gradio tunnel
# ---------------------------------------------------------------------------
@app.function(
    gpu="A10G",          # 24GB VRAM — minimum for Moshi 7B
    timeout=7200,        # 2 hours max session
    scaledown_window=300,
)
def run_server():
    """Launch J-Moshi server with Gradio tunnel for public access."""
    import os
    import re
    import tempfile

    print("Starting J-Moshi server...")
    print(f"GPU: {os.popen('nvidia-smi --query-gpu=name --format=csv,noheader').read().strip()}")

    ssl_dir = tempfile.mkdtemp()

    cmd = [
        "python", "-m", "moshi.server",
        "--hf-repo", "nu-dialogue/j-moshi",
        "--gradio-tunnel",
        "--ssl", ssl_dir,
        "--device", "cuda",
        "--host", "0.0.0.0",
        "--port", "8998",
    ]

    print(f"Command: {' '.join(cmd)}")

    process = subprocess.Popen(
        cmd,
        env=os.environ.copy(),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        universal_newlines=True,
        bufsize=1,
    )

    gradio_url = None
    for line in process.stdout:
        print(line.rstrip())

        # Extract Gradio tunnel URL
        if "Running on public URL:" in line or "gradio.live" in line:
            match = re.search(r"https://[a-zA-Z0-9\-]+\.gradio\.live", line)
            if match:
                gradio_url = match.group(0)
                print(f"\n=== J-Moshi ready ===")
                print(f"URL: {gradio_url}")
                print(f"Use headphones to avoid echo.\n")
                break

        if "Access the Web UI" in line:
            print("Waiting for tunnel URL...")

    if gradio_url:
        try:
            process.wait()
        except KeyboardInterrupt:
            print("Stopping server...")
            process.terminate()
            process.wait()
    else:
        print("Failed to get Gradio tunnel URL. Check logs.")
        # Keep running anyway — might still be accessible
        try:
            process.wait()
        except KeyboardInterrupt:
            process.terminate()
            process.wait()

    return gradio_url


# ---------------------------------------------------------------------------
# CLI entrypoint: modal run j_moshi.py
# ---------------------------------------------------------------------------
@app.local_entrypoint()
def main():
    """Launch J-Moshi on Modal from local machine."""
    print("J-Moshi on Modal")
    print("Starting... (model load takes ~1-2 min on first run)\n")

    url = run_server.remote()

    if url:
        print(f"\nServer running at: {url}")
        print("Open in browser. Use headphones.")
    else:
        print("\nServer may have started without tunnel URL.")
        print("Check modal app logs j-moshi")
        sys.exit(1)
