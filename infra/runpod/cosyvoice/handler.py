"""
CosyVoice 2 TTS — RunPod Serverless Handler
Supports: zero_shot, cross_lingual, instruct, sft modes
"""
import runpod
import base64
import io
import os
import sys
import torch
import torchaudio

sys.path.append("/app")
sys.path.append("/app/third_party/Matcha-TTS")

from cosyvoice.cli.cosyvoice import CosyVoice2

# Load model at container startup (outside handler for fast warm starts)
MODEL_DIR = os.environ.get("MODEL_DIR", "pretrained_models/CosyVoice2-0.5B")
print(f"Loading CosyVoice2 from {MODEL_DIR}...")
cosyvoice = CosyVoice2(MODEL_DIR, load_jit=False, load_trt=False)
print(f"CosyVoice2 loaded. Sample rate: {cosyvoice.sample_rate}")

# Pre-load default speaker embeddings if available
SPEAKERS_DIR = os.environ.get("SPEAKERS_DIR", "/data/speakers")


def load_audio_from_input(audio_data):
    """Load audio from base64 string or URL."""
    if not audio_data:
        return None

    if audio_data.startswith("http://") or audio_data.startswith("https://"):
        import requests
        resp = requests.get(audio_data, timeout=30)
        resp.raise_for_status()
        buf = io.BytesIO(resp.content)
    else:
        # Assume base64
        raw = base64.b64decode(audio_data)
        buf = io.BytesIO(raw)

    waveform, sr = torchaudio.load(buf)
    # Resample to 16kHz if needed
    if sr != 16000:
        resampler = torchaudio.transforms.Resample(sr, 16000)
        waveform = resampler(waveform)
    return waveform


def handler(event):
    """Main handler for RunPod serverless."""
    input_data = event.get("input", {})

    text = input_data.get("text", "")
    mode = input_data.get("mode", "sft")  # sft, zero_shot, cross_lingual, instruct
    speaker_id = input_data.get("speaker_id", "")
    prompt_text = input_data.get("prompt_text", "")
    prompt_audio = input_data.get("prompt_audio", "")  # base64 or URL
    instruct_text = input_data.get("instruct_text", "")
    speed = float(input_data.get("speed", 1.0))
    output_format = input_data.get("format", "wav")  # wav or mp3

    if not text:
        return {"error": "text is required"}

    try:
        # Run inference based on mode
        if mode == "zero_shot":
            if not prompt_audio:
                return {"error": "prompt_audio is required for zero_shot mode"}
            prompt_wav = load_audio_from_input(prompt_audio)
            # Save to temp file (CosyVoice expects file path)
            tmp_path = "/tmp/prompt.wav"
            torchaudio.save(tmp_path, prompt_wav, 16000)
            results = list(cosyvoice.inference_zero_shot(
                text, prompt_text, tmp_path, stream=False
            ))
        elif mode == "cross_lingual":
            if not prompt_audio:
                return {"error": "prompt_audio is required for cross_lingual mode"}
            prompt_wav = load_audio_from_input(prompt_audio)
            tmp_path = "/tmp/prompt.wav"
            torchaudio.save(tmp_path, prompt_wav, 16000)
            results = list(cosyvoice.inference_cross_lingual(
                text, tmp_path, stream=False
            ))
        elif mode == "instruct":
            if not prompt_audio or not instruct_text:
                return {"error": "prompt_audio and instruct_text required for instruct mode"}
            prompt_wav = load_audio_from_input(prompt_audio)
            tmp_path = "/tmp/prompt.wav"
            torchaudio.save(tmp_path, prompt_wav, 16000)
            results = list(cosyvoice.inference_instruct2(
                text, instruct_text, tmp_path, stream=False
            ))
        else:
            # SFT mode — use built-in speaker
            spk = speaker_id or cosyvoice.list_available_spks()[0]
            results = list(cosyvoice.inference_sft(
                text, spk, stream=False
            ))

        if not results:
            return {"error": "No audio generated"}

        # Combine audio chunks
        audio_chunks = [r["tts_speech"] for r in results]
        combined = torch.cat(audio_chunks, dim=1)

        # Apply speed adjustment if needed
        if speed != 1.0 and speed > 0:
            effects = [["tempo", str(speed)]]
            combined, _ = torchaudio.sox_effects.apply_effects_tensor(
                combined, cosyvoice.sample_rate, effects
            )

        # Encode to output format
        buffer = io.BytesIO()
        if output_format == "mp3":
            torchaudio.save(buffer, combined, cosyvoice.sample_rate, format="mp3")
            content_type = "audio/mpeg"
        else:
            torchaudio.save(buffer, combined, cosyvoice.sample_rate, format="wav")
            content_type = "audio/wav"

        audio_b64 = base64.b64encode(buffer.getvalue()).decode("utf-8")

        return {
            "audio_base64": audio_b64,
            "sample_rate": cosyvoice.sample_rate,
            "format": output_format,
            "content_type": content_type,
            "duration_ms": int(combined.shape[1] / cosyvoice.sample_rate * 1000),
        }

    except Exception as e:
        return {"error": str(e)}


# For listing available speakers
def info_handler(event):
    """Return model info and available speakers."""
    return {
        "model": "CosyVoice2-0.5B",
        "sample_rate": cosyvoice.sample_rate,
        "available_speakers": cosyvoice.list_available_spks(),
        "modes": ["sft", "zero_shot", "cross_lingual", "instruct"],
    }


runpod.serverless.start({
    "handler": handler,
})
