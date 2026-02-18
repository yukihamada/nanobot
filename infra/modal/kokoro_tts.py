"""
Kokoro TTS on Modal — Fast, multilingual text-to-speech service.

Deploy:  modal deploy kokoro_tts.py
Test:    modal run kokoro_tts.py
Logs:    modal app logs kokoro-tts

Exposes:
  POST /synthesize  — JSON body: { text, voice?, lang?, speed?, format? }
                      Returns: audio/mpeg or audio/wav bytes
  GET  /health      — Returns model status
  GET  /voices      — Lists available voices per language
"""
import io
import modal

# ---------------------------------------------------------------------------
# Modal image: install Kokoro + espeak-ng + MeCab (for Japanese) + ffmpeg
# ---------------------------------------------------------------------------
kokoro_image = (
    modal.Image.debian_slim(python_version="3.11")
    .apt_install(
        "espeak-ng", "ffmpeg", "libsndfile1",
        # MeCab for Japanese tokenization (required by fugashi/unidic)
        "mecab", "libmecab-dev", "mecab-ipadic-utf8",
    )
    .pip_install(
        "kokoro>=0.9.4",
        "misaki[en,ja,zh]",
        "soundfile",
        "torch",
        "numpy",
        "pydub",
        "fugashi[unidic]",
        "fastapi[standard]",
    )
    # Download unidic dictionary (required by fugashi for Japanese tokenization)
    .run_commands("python3 -m unidic download")
    # Pre-download model weights and validate pipelines during image build
    .run_commands(
        'python3 -c "'
        "from kokoro import KPipeline; "
        "p = KPipeline(lang_code='a'); "
        "print('EN pipeline ready'); "
        "p2 = KPipeline(lang_code='j'); "
        "print('JA pipeline ready')"
        '"'
    )
)

app = modal.App("kokoro-tts", image=kokoro_image)

# ---------------------------------------------------------------------------
# Voices mapping — top voices per language
# ---------------------------------------------------------------------------
VOICE_MAP = {
    # American English
    "a": {
        "default": "af_heart",
        "nova": "af_heart",        # Female, warm
        "alloy": "af_bella",       # Female, neutral
        "echo": "am_adam",         # Male, clear
        "onyx": "am_michael",     # Male, deep
        "shimmer": "af_nicole",   # Female, bright
    },
    # British English
    "b": {
        "default": "bf_emma",
        "nova": "bf_emma",
        "echo": "bm_george",
    },
    # Japanese
    "j": {
        "default": "jf_alpha",
        "nova": "jf_alpha",       # Female Japanese
        "alloy": "jf_gongitsune", # Female Japanese alt
        "echo": "jm_kumo",        # Male Japanese
        "onyx": "jm_kumo",
    },
    # Mandarin Chinese
    "z": {
        "default": "zf_xiaobei",
        "nova": "zf_xiaobei",
        "echo": "zm_yunjian",
    },
}


def detect_lang(text: str) -> str:
    """Detect language code from text content."""
    has_kana = any(
        '\u3040' <= c <= '\u309F' or '\u30A0' <= c <= '\u30FF'
        for c in text
    )
    cjk_count = sum(1 for c in text if '\u4E00' <= c <= '\u9FFF')

    if has_kana or (cjk_count > 0 and any(
        '\u3040' <= c <= '\u30FF' for c in text
    )):
        return "j"
    if cjk_count > len(text) * 0.2:
        return "z"
    return "a"


def resolve_voice(lang: str, voice_name: str) -> str:
    """Resolve OpenAI-style voice name to Kokoro voice ID."""
    lang_voices = VOICE_MAP.get(lang, VOICE_MAP["a"])
    # If it looks like a Kokoro voice ID (e.g. "af_heart"), use directly
    if "_" in voice_name and len(voice_name) > 3:
        return voice_name
    return lang_voices.get(voice_name, lang_voices["default"])


# ---------------------------------------------------------------------------
# Web endpoint — FastAPI ASGI app on GPU
# To keep 1 container always warm (eliminates cold starts, ~$0.13/hr for T4):
#   Change min_containers=0 to min_containers=1
# ---------------------------------------------------------------------------
@app.function(
    gpu="T4",
    min_containers=0,        # Set to 1 for always-warm (no cold starts)
    scaledown_window=300,    # Keep container alive 5 min after last request
    image=kokoro_image,
)
@modal.concurrent(max_inputs=4)
@modal.asgi_app()
def web():
    """FastAPI web endpoint for TTS — loads model once at container startup."""
    from fastapi import FastAPI, HTTPException, Response
    from fastapi.middleware.cors import CORSMiddleware
    from pydantic import BaseModel, Field
    from typing import Optional
    import numpy as np
    import soundfile as sf
    import torch
    from kokoro import KPipeline, KModel

    fastapi_app = FastAPI(title="Kokoro TTS", version="1.0")

    fastapi_app.add_middleware(
        CORSMiddleware,
        allow_origins=["*"],
        allow_methods=["*"],
        allow_headers=["*"],
    )

    class SynthesizeRequest(BaseModel):
        text: str = ""
        voice: str = Field(default="nova")
        lang: str = Field(default="")
        speed: float = Field(default=1.0, ge=0.25, le=4.0)
        format: str = Field(default="mp3")
        # OpenAI-compatible fields
        input: Optional[str] = None
        model: Optional[str] = None
        instructions: Optional[str] = None

    # ---- Eager model loading at import time (container startup) ----
    _device = "cuda" if torch.cuda.is_available() else "cpu"
    print(f"Loading Kokoro model on {_device}...")
    _model = KModel(repo_id="hexgrad/Kokoro-82M").to(_device).eval()
    _pipelines: dict = {}
    for _lc in ["a", "j"]:
        try:
            _pipelines[_lc] = KPipeline(lang_code=_lc, model=_model, device=_device)
            print(f"  Pipeline '{_lc}' ready")
        except Exception as _e:
            print(f"  Pipeline '{_lc}' failed: {_e}")
    print("Kokoro TTS loaded")

    def _get_pipeline(lang: str):
        if lang not in _pipelines:
            _pipelines[lang] = KPipeline(lang_code=lang, model=_model, device=_device)
        return _pipelines[lang]

    def do_synthesize(text: str, voice: str, lang: str, speed: float, output_format: str):
        if not lang:
            lang = detect_lang(text)
        voice_id = resolve_voice(lang, voice)
        pipeline = _get_pipeline(lang)

        audio_chunks = []
        for result in pipeline(text, voice=voice_id, speed=speed):
            if hasattr(result, "audio") and result.audio is not None:
                if isinstance(result.audio, torch.Tensor):
                    audio_chunks.append(result.audio.cpu().numpy())
                else:
                    audio_chunks.append(np.array(result.audio))

        if not audio_chunks:
            raise RuntimeError("No audio generated")

        audio = np.concatenate(audio_chunks)
        buffer = io.BytesIO()
        if output_format == "wav":
            sf.write(buffer, audio, 24000, format="WAV")
            content_type = "audio/wav"
        else:
            wav_buf = io.BytesIO()
            sf.write(wav_buf, audio, 24000, format="WAV")
            wav_buf.seek(0)
            from pydub import AudioSegment
            seg = AudioSegment.from_wav(wav_buf)
            seg.export(buffer, format="mp3", bitrate="128k")
            content_type = "audio/mpeg"

        return buffer.getvalue(), content_type

    @fastapi_app.post("/synthesize")
    async def synthesize(req: SynthesizeRequest):
        text = req.input or req.text
        if not text or not text.strip():
            raise HTTPException(400, "text is required")
        if len(text) > 4096:
            raise HTTPException(400, "text must be under 4096 characters")
        try:
            audio_bytes, content_type = do_synthesize(
                text, req.voice, req.lang, req.speed, req.format
            )
            return Response(
                content=audio_bytes,
                media_type=content_type,
                headers={
                    "Content-Disposition": f'inline; filename="speech.{req.format}"',
                },
            )
        except Exception as e:
            raise HTTPException(500, f"TTS failed: {str(e)}")

    # OpenAI-compatible endpoint
    @fastapi_app.post("/v1/audio/speech")
    async def openai_compat(req: SynthesizeRequest):
        text = req.input or req.text
        if not text or not text.strip():
            raise HTTPException(400, "input is required")
        if len(text) > 4096:
            raise HTTPException(400, "input must be under 4096 characters")
        try:
            audio_bytes, content_type = do_synthesize(
                text, req.voice, req.lang, req.speed, req.format
            )
            return Response(content=audio_bytes, media_type=content_type)
        except Exception as e:
            raise HTTPException(500, f"TTS failed: {str(e)}")

    @fastapi_app.get("/health")
    async def health():
        return {
            "status": "ok",
            "model": "Kokoro-82M",
            "device": _device,
            "languages": list(_pipelines.keys()),
        }

    @fastapi_app.get("/voices")
    async def voices():
        return VOICE_MAP

    return fastapi_app


# ---------------------------------------------------------------------------
# CLI: quick test via `modal run kokoro_tts.py`
# ---------------------------------------------------------------------------
@app.local_entrypoint()
def main():
    """Test the TTS web endpoint by calling the Modal function directly."""
    print("Deploy with: modal deploy kokoro_tts.py")
    print("Then test with:")
    print('  curl -X POST https://<your-url>/synthesize \\')
    print('    -H "Content-Type: application/json" \\')
    print('    -d \'{"text": "Hello world"}\' --output test.mp3')
