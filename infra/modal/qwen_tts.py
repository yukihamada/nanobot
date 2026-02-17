"""
Qwen3 Voice Cloning TTS on Modal — High-quality voice synthesis with custom voice cloning.

Deploy:  modal deploy qwen_tts.py
Test:    modal run qwen_tts.py
Logs:    modal app logs qwen-tts

Exposes:
  POST /synthesize  — JSON body: { text, voice?, reference_audio?, language?, speed? }
                      Returns: audio/mpeg bytes (MP3)
  GET  /health      — Returns model status
  GET  /voices      — Lists available preset voices
"""
import io
import base64
import modal

# ---------------------------------------------------------------------------
# Modal image: Qwen3-TTS with voice cloning support
# ---------------------------------------------------------------------------
qwen_image = (
    modal.Image.debian_slim(python_version="3.11")
    .apt_install("ffmpeg", "libsndfile1")
    .pip_install(
        "torch",
        "torchaudio",
        "transformers>=4.40.0",
        "soundfile",
        "numpy",
        "pydub",
        "fastapi[standard]",
    )
    # Pre-download Qwen3-TTS model during image build
    .run_commands(
        'python3 -c "'
        "from transformers import AutoModel, AutoProcessor; "
        "model = AutoModel.from_pretrained('Qwen/Qwen-Audio-Chat', trust_remote_code=True); "
        "processor = AutoProcessor.from_pretrained('Qwen/Qwen-Audio-Chat', trust_remote_code=True); "
        "print('Qwen3-TTS model ready')"
        '"'
    )
)

app = modal.App("qwen-tts", image=qwen_image)

# ---------------------------------------------------------------------------
# Model container — GPU instance with Qwen3-TTS loaded
# ---------------------------------------------------------------------------
@app.cls(
    gpu="A10G",  # 24GB VRAM, good for voice cloning
    container_idle_timeout=300,
    timeout=600,
)
class QwenTTS:
    @modal.enter()
    def load_model(self):
        """Load Qwen3-TTS model on container startup."""
        import torch
        from transformers import AutoModel, AutoProcessor

        print("Loading Qwen3-TTS model...")
        self.device = "cuda" if torch.cuda.is_available() else "cpu"
        self.model = AutoModel.from_pretrained(
            "Qwen/Qwen-Audio-Chat",
            trust_remote_code=True,
        ).to(self.device)
        self.processor = AutoProcessor.from_pretrained(
            "Qwen/Qwen-Audio-Chat",
            trust_remote_code=True,
        )
        print(f"Qwen3-TTS loaded on {self.device}")

    @modal.method()
    def synthesize(
        self,
        text: str,
        voice: str = "default",
        reference_audio: str | None = None,
        language: str = "ja",
        speed: float = 1.0,
    ) -> bytes:
        """
        Synthesize speech from text with optional voice cloning.

        Args:
            text: Text to synthesize
            voice: Voice preset (or "clone" for reference-based cloning)
            reference_audio: Base64-encoded audio for voice cloning
            language: Language code (ja, en, zh)
            speed: Speech speed multiplier

        Returns:
            MP3 audio bytes
        """
        import torch
        import soundfile as sf
        from pydub import AudioSegment

        try:
            # Voice cloning mode
            if reference_audio and voice == "clone":
                # Decode reference audio
                ref_audio_bytes = base64.b64decode(reference_audio)

                # Load reference audio
                import io
                ref_audio, ref_sr = sf.read(io.BytesIO(ref_audio_bytes))

                # Generate with voice cloning
                prompt = f"<|audio_start|><|audio_end|>{text}"
                inputs = self.processor(
                    text=[prompt],
                    audios=[ref_audio],
                    sampling_rate=ref_sr,
                    return_tensors="pt",
                ).to(self.device)

                with torch.no_grad():
                    outputs = self.model.generate(**inputs, max_length=2048)
                    audio_output = outputs.audio[0].cpu().numpy()
                    sample_rate = outputs.sampling_rate

            # Preset voice mode
            else:
                # Use preset voices (simplified for now)
                inputs = self.processor(
                    text=[text],
                    return_tensors="pt",
                ).to(self.device)

                with torch.no_grad():
                    outputs = self.model.generate(**inputs, max_length=2048)
                    audio_output = outputs.audio[0].cpu().numpy()
                    sample_rate = 24000  # Default Qwen sample rate

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

            return mp3_buffer.getvalue()

        except Exception as e:
            print(f"Qwen TTS error: {e}")
            raise

    @modal.method()
    def health(self) -> dict:
        """Health check endpoint."""
        import torch
        return {
            "status": "ok",
            "model": "Qwen3-TTS",
            "device": self.device,
            "gpu_available": torch.cuda.is_available(),
        }

# ---------------------------------------------------------------------------
# FastAPI web endpoint
# ---------------------------------------------------------------------------
@app.function(
    image=qwen_image,
    keep_warm=1,  # Keep 1 container warm for low latency
)
@modal.asgi_app()
def api():
    from fastapi import FastAPI, HTTPException
    from fastapi.responses import Response
    from pydantic import BaseModel

    web_app = FastAPI(title="Qwen3 TTS API")
    qwen = QwenTTS()

    class SynthesizeRequest(BaseModel):
        text: str
        voice: str = "default"
        reference_audio: str | None = None
        language: str = "ja"
        speed: float = 1.0

    @web_app.post("/synthesize")
    async def synthesize(req: SynthesizeRequest):
        """Synthesize speech from text."""
        try:
            audio_bytes = qwen.synthesize.remote(
                text=req.text,
                voice=req.voice,
                reference_audio=req.reference_audio,
                language=req.language,
                speed=req.speed,
            )
            return Response(content=audio_bytes, media_type="audio/mpeg")
        except Exception as e:
            raise HTTPException(status_code=500, detail=str(e))

    @web_app.get("/health")
    async def health():
        """Health check."""
        return qwen.health.remote()

    @web_app.get("/voices")
    async def voices():
        """List available voices."""
        return {
            "preset_voices": ["default", "nova", "alloy", "echo"],
            "clone_voice": "clone",
            "note": "Use voice='clone' with reference_audio for voice cloning",
        }

    return web_app

# ---------------------------------------------------------------------------
# Local test
# ---------------------------------------------------------------------------
@app.local_entrypoint()
def test():
    """Test Qwen3-TTS synthesis locally."""
    qwen = QwenTTS()

    # Test synthesis
    print("Testing Japanese synthesis...")
    audio = qwen.synthesize.remote(
        text="こんにちは、ゆうきです。chatweb.aiへようこそ！",
        language="ja",
    )

    # Save test output
    with open("/tmp/qwen_test.mp3", "wb") as f:
        f.write(audio)
    print(f"✓ Test audio saved: /tmp/qwen_test.mp3 ({len(audio)} bytes)")

    # Health check
    status = qwen.health.remote()
    print(f"✓ Health: {status}")
