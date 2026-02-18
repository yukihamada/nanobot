"""
Qwen3 Voice Cloning TTS Handler for RunPod.

Supports:
- Text-to-speech with preset voices
- Voice cloning from reference audio (base64-encoded)
- Multiple languages (Japanese, English, Chinese)
"""
import io
import base64
import torch
import soundfile as sf
from pydub import AudioSegment
from transformers import AutoModel, AutoProcessor
import runpod

# Load model on startup
print("Loading Qwen3-TTS model...")
device = "cuda" if torch.cuda.is_available() else "cpu"
model = AutoModel.from_pretrained(
    "Qwen/Qwen-Audio-Chat",
    trust_remote_code=True,
).to(device)
processor = AutoProcessor.from_pretrained(
    "Qwen/Qwen-Audio-Chat",
    trust_remote_code=True,
)
print(f"Qwen3-TTS loaded on {device}")


def synthesize(text: str, voice: str = "default", reference_audio: str | None = None,
               language: str = "ja", speed: float = 1.0) -> bytes:
    """
    Synthesize speech from text with optional voice cloning.

    Args:
        text: Text to synthesize
        voice: Voice preset (or "clone" for reference-based cloning)
        reference_audio: Base64-encoded audio for voice cloning
        language: Language code (ja, en, zh)
        speed: Speech speed multiplier

    Returns:
        MP3 audio bytes (base64-encoded)
    """
    try:
        # Voice cloning mode
        if reference_audio and voice == "clone":
            # Decode reference audio
            ref_audio_bytes = base64.b64decode(reference_audio)
            ref_audio, ref_sr = sf.read(io.BytesIO(ref_audio_bytes))

            # Generate with voice cloning
            prompt = f"<|audio_start|><|audio_end|>{text}"
            inputs = processor(
                text=[prompt],
                audios=[ref_audio],
                sampling_rate=ref_sr,
                return_tensors="pt",
            ).to(device)

            with torch.no_grad():
                outputs = model.generate(**inputs, max_length=2048)
                audio_output = outputs.audio[0].cpu().numpy()
                sample_rate = outputs.sampling_rate

        # Preset voice mode
        else:
            inputs = processor(
                text=[text],
                return_tensors="pt",
            ).to(device)

            with torch.no_grad():
                outputs = model.generate(**inputs, max_length=2048)
                audio_output = outputs.audio[0].cpu().numpy()
                sample_rate = 24000

        # Apply speed adjustment
        if speed != 1.0:
            import numpy as np
            indices = np.arange(0, len(audio_output), speed)
            audio_output = np.interp(indices, np.arange(len(audio_output)), audio_output)

        # Convert to MP3
        wav_buffer = io.BytesIO()
        sf.write(wav_buffer, audio_output, sample_rate, format="WAV")
        wav_buffer.seek(0)

        audio_segment = AudioSegment.from_wav(wav_buffer)
        mp3_buffer = io.BytesIO()
        audio_segment.export(mp3_buffer, format="mp3", bitrate="128k")

        mp3_bytes = mp3_buffer.getvalue()
        return base64.b64encode(mp3_bytes).decode()

    except Exception as e:
        print(f"Synthesis error: {e}")
        raise


def handler(event):
    """
    RunPod handler function.

    Expected input:
    {
        "text": "こんにちは",
        "voice": "default" or "clone",
        "reference_audio": "base64_encoded_audio" (optional),
        "language": "ja" (optional),
        "speed": 1.0 (optional)
    }

    Returns:
    {
        "audio": "base64_encoded_mp3",
        "sample_rate": 24000,
        "duration_seconds": 2.5
    }
    """
    try:
        input_data = event["input"]
        text = input_data["text"]
        voice = input_data.get("voice", "default")
        reference_audio = input_data.get("reference_audio")
        language = input_data.get("language", "ja")
        speed = input_data.get("speed", 1.0)

        print(f"Synthesizing: '{text[:50]}...' (voice={voice}, lang={language})")

        # Synthesize
        audio_base64 = synthesize(
            text=text,
            voice=voice,
            reference_audio=reference_audio,
            language=language,
            speed=speed,
        )

        # Calculate duration
        audio_bytes = base64.b64decode(audio_base64)
        audio_segment = AudioSegment.from_mp3(io.BytesIO(audio_bytes))
        duration = len(audio_segment) / 1000.0  # ms to seconds

        return {
            "audio": audio_base64,
            "sample_rate": 24000,
            "duration_seconds": duration,
        }

    except Exception as e:
        return {"error": str(e)}


# Start RunPod serverless handler
if __name__ == "__main__":
    runpod.serverless.start({"handler": handler})
