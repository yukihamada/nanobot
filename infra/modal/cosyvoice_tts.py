"""
CosyVoice 2 TTS on Modal — High-quality voice cloning and multilingual synthesis.

Deploy:  modal deploy cosyvoice_tts.py
Test:    modal run cosyvoice_tts.py
Logs:    modal app logs cosyvoice-tts

Exposes:
  POST /synthesize  — JSON body: { text, mode?, prompt_audio?, prompt_text?, speaker_id?, speed? }
                      Returns: audio/mpeg or audio/wav bytes
  GET  /health      — Returns model status
  GET  /speakers    — Lists available speakers
"""
import io
import base64
import modal

# ---------------------------------------------------------------------------
# Modal image: CosyVoice 2 with all dependencies
# ---------------------------------------------------------------------------
cosyvoice_image = (
    modal.Image.debian_slim(python_version="3.11")
    .apt_install("git", "ffmpeg", "libsndfile1", "sox")
    .pip_install(
        "setuptools",  # Required for openai-whisper build
        "torch",
        "torchaudio",
        "soundfile",
        "numpy",
        "pydub",
        "fastapi[standard]",
        "onnxruntime",
        "modelscope",
        "huggingface_hub",
    )
    # Clone CosyVoice repository
    .run_commands(
        "cd /tmp && git clone --depth=1 https://github.com/FunAudioLLM/CosyVoice.git",
        "cd /tmp/CosyVoice && pip install --no-deps -r requirements.txt || true",  # Skip deps, we install manually
    )
    # Download model weights during build
    .run_commands(
        'python3 -c "'
        "from modelscope import snapshot_download; "
        "snapshot_download('iic/CosyVoice2-0.5B', cache_dir='/root/.cache/modelscope'); "
        "print('CosyVoice2 model cached')"
        '"'
    )
)

app = modal.App("cosyvoice-tts", image=cosyvoice_image)

# ---------------------------------------------------------------------------
# Model container — GPU instance with CosyVoice loaded
# ---------------------------------------------------------------------------
@app.cls(
    gpu="A10G",  # 24GB VRAM
    scaledown_window=300,  # Keep warm for 5 min
    timeout=600,
)
class CosyVoiceTTS:
    @modal.enter()
    def load_model(self):
        """Load CosyVoice model on container startup."""
        import sys
        sys.path.append("/tmp/CosyVoice")
        sys.path.append("/tmp/CosyVoice/third_party/Matcha-TTS")

        from cosyvoice.cli.cosyvoice import CosyVoice2

        print("Loading CosyVoice2 model...")
        model_dir = "/root/.cache/modelscope/iic/CosyVoice2-0___5B"
        self.cosyvoice = CosyVoice2(model_dir, load_jit=False, load_trt=False)
        self.sample_rate = self.cosyvoice.sample_rate
        print(f"CosyVoice2 loaded. Sample rate: {self.sample_rate}")

    def _load_audio(self, audio_data: str | None) -> tuple | None:
        """Load audio from base64 string."""
        if not audio_data:
            return None

        import torchaudio

        # Decode base64
        raw = base64.b64decode(audio_data)
        buf = io.BytesIO(raw)

        waveform, sr = torchaudio.load(buf)

        # Resample to 16kHz if needed
        if sr != 16000:
            resampler = torchaudio.transforms.Resample(sr, 16000)
            waveform = resampler(waveform)

        return waveform, 16000

    @modal.method()
    def synthesize(
        self,
        text: str,
        mode: str = "sft",
        speaker_id: str = "",
        prompt_audio: str | None = None,
        prompt_text: str = "",
        instruct_text: str = "",
        speed: float = 1.0,
        output_format: str = "mp3",
    ) -> bytes:
        """
        Synthesize speech from text.

        Args:
            text: Text to synthesize
            mode: "sft" (preset voices), "zero_shot" (voice cloning),
                  "cross_lingual" (multilingual), "instruct" (style control)
            speaker_id: Speaker ID for SFT mode
            prompt_audio: Base64-encoded reference audio for cloning
            prompt_text: Transcript of prompt audio (for zero_shot)
            instruct_text: Style instruction (for instruct mode)
            speed: Speech speed multiplier
            output_format: "mp3" or "wav"

        Returns:
            Audio bytes
        """
        import torch
        import torchaudio

        try:
            # Run inference based on mode
            if mode == "zero_shot":
                if not prompt_audio:
                    raise ValueError("prompt_audio required for zero_shot mode")

                waveform, sr = self._load_audio(prompt_audio)

                # Save to temp file
                tmp_path = "/tmp/prompt.wav"
                torchaudio.save(tmp_path, waveform, sr)

                results = list(self.cosyvoice.inference_zero_shot(
                    text, prompt_text, tmp_path, stream=False
                ))

            elif mode == "cross_lingual":
                if not prompt_audio:
                    raise ValueError("prompt_audio required for cross_lingual mode")

                waveform, sr = self._load_audio(prompt_audio)
                tmp_path = "/tmp/prompt.wav"
                torchaudio.save(tmp_path, waveform, sr)

                results = list(self.cosyvoice.inference_cross_lingual(
                    text, tmp_path, stream=False
                ))

            elif mode == "instruct":
                if not prompt_audio or not instruct_text:
                    raise ValueError("prompt_audio and instruct_text required")

                waveform, sr = self._load_audio(prompt_audio)
                tmp_path = "/tmp/prompt.wav"
                torchaudio.save(tmp_path, waveform, sr)

                results = list(self.cosyvoice.inference_instruct2(
                    text, instruct_text, tmp_path, stream=False
                ))

            else:  # SFT mode
                spk = speaker_id or self.cosyvoice.list_available_spks()[0]
                results = list(self.cosyvoice.inference_sft(
                    text, spk, stream=False
                ))

            if not results:
                raise ValueError("No audio generated")

            # Combine audio chunks
            audio_chunks = [r["tts_speech"] for r in results]
            combined = torch.cat(audio_chunks, dim=1)

            # Apply speed adjustment
            if speed != 1.0 and speed > 0:
                effects = [["tempo", str(speed)]]
                combined, _ = torchaudio.sox_effects.apply_effects_tensor(
                    combined, self.sample_rate, effects
                )

            # Encode to output format
            buffer = io.BytesIO()
            if output_format == "mp3":
                torchaudio.save(buffer, combined, self.sample_rate, format="mp3")
            else:
                torchaudio.save(buffer, combined, self.sample_rate, format="wav")

            return buffer.getvalue()

        except Exception as e:
            print(f"CosyVoice synthesis error: {e}")
            raise

    @modal.method()
    def get_speakers(self) -> list[str]:
        """List available preset speakers."""
        return self.cosyvoice.list_available_spks()

    @modal.method()
    def health(self) -> dict:
        """Health check."""
        import torch
        return {
            "status": "ok",
            "model": "CosyVoice2-0.5B",
            "sample_rate": self.sample_rate,
            "gpu_available": torch.cuda.is_available(),
            "modes": ["sft", "zero_shot", "cross_lingual", "instruct"],
        }

# ---------------------------------------------------------------------------
# FastAPI web endpoint
# ---------------------------------------------------------------------------
@app.function(
    image=cosyvoice_image,
    min_containers=1,  # Keep warm
)
@modal.asgi_app()
def api():
    from fastapi import FastAPI, HTTPException
    from fastapi.responses import Response
    from pydantic import BaseModel

    web_app = FastAPI(title="CosyVoice TTS API")
    cosyvoice_tts = CosyVoiceTTS()

    class SynthesizeRequest(BaseModel):
        text: str
        mode: str = "sft"
        speaker_id: str = ""
        prompt_audio: str | None = None
        prompt_text: str = ""
        instruct_text: str = ""
        speed: float = 1.0
        output_format: str = "mp3"

    @web_app.post("/synthesize")
    async def synthesize(req: SynthesizeRequest):
        """Synthesize speech from text."""
        try:
            audio_bytes = cosyvoice_tts.synthesize.remote(
                text=req.text,
                mode=req.mode,
                speaker_id=req.speaker_id,
                prompt_audio=req.prompt_audio,
                prompt_text=req.prompt_text,
                instruct_text=req.instruct_text,
                speed=req.speed,
                output_format=req.output_format,
            )
            media_type = "audio/mpeg" if req.output_format == "mp3" else "audio/wav"
            return Response(content=audio_bytes, media_type=media_type)
        except Exception as e:
            raise HTTPException(status_code=500, detail=str(e))

    @web_app.get("/speakers")
    async def speakers():
        """List available speakers."""
        return {"speakers": cosyvoice_tts.get_speakers.remote()}

    @web_app.get("/health")
    async def health():
        """Health check."""
        return cosyvoice_tts.health.remote()

    return web_app

# ---------------------------------------------------------------------------
# Local test
# ---------------------------------------------------------------------------
@app.local_entrypoint()
def test():
    """Test CosyVoice synthesis locally."""
    tts = CosyVoiceTTS()

    # Test SFT mode
    print("Testing SFT mode (preset voice)...")
    audio = tts.synthesize.remote(
        text="こんにちは、ゆうきです。CosyVoice音声合成のテストです。",
        mode="sft",
    )

    with open("/tmp/cosyvoice_sft_test.mp3", "wb") as f:
        f.write(audio)
    print(f"✓ SFT test audio saved: /tmp/cosyvoice_sft_test.mp3 ({len(audio)} bytes)")

    # Health check
    status = tts.health.remote()
    print(f"✓ Health: {status}")

    # List speakers
    speakers = tts.get_speakers.remote()
    print(f"✓ Available speakers: {speakers[:5]}...")  # First 5
