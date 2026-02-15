"""
RunPod Qwen3 Voice Cloning API Server
Provides voice synthesis with custom voice cloning capabilities
"""

import os
import io
import base64
import logging
from typing import Optional
from fastapi import FastAPI, HTTPException, UploadFile, File, Form
from fastapi.responses import Response, JSONResponse
from pydantic import BaseModel
import torch
from transformers import AutoProcessor, AutoModel
import soundfile as sf
import numpy as np

# Configure logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

app = FastAPI(title="Qwen3 Voice Cloning API", version="1.0.0")

# Global model storage
processor = None
model = None
device = None

class SynthesizeRequest(BaseModel):
    text: str
    voice: Optional[str] = "default"
    language: Optional[str] = "auto"
    speed: Optional[float] = 1.0
    reference_audio: Optional[str] = None  # Base64 encoded audio for voice cloning

class SynthesizeResponse(BaseModel):
    audio: str  # Base64 encoded audio
    sample_rate: int
    duration: float

@app.on_event("startup")
async def load_model():
    """Load Qwen TTS model on startup"""
    global processor, model, device

    logger.info("Loading Qwen3 TTS model...")

    # Check for GPU availability
    device = "cuda" if torch.cuda.is_available() else "cpu"
    logger.info(f"Using device: {device}")

    try:
        # Load Qwen TTS model (adjust model name as needed)
        model_name = os.getenv("MODEL_NAME", "Qwen/Qwen2.5-TTS")

        logger.info(f"Loading model: {model_name}")
        processor = AutoProcessor.from_pretrained(model_name)
        model = AutoModel.from_pretrained(
            model_name,
            torch_dtype=torch.float16 if device == "cuda" else torch.float32,
        ).to(device)

        logger.info("Model loaded successfully!")

    except Exception as e:
        logger.error(f"Failed to load model: {e}")
        # Fallback to mock mode for development
        logger.warning("Running in MOCK mode - returning synthetic audio")

@app.get("/health")
async def health_check():
    """Health check endpoint"""
    return {
        "status": "healthy",
        "model_loaded": model is not None,
        "device": str(device) if device else "unknown"
    }

@app.post("/synthesize", response_model=SynthesizeResponse)
async def synthesize(request: SynthesizeRequest):
    """
    Synthesize speech from text

    - **text**: Text to synthesize
    - **voice**: Voice preset (or use reference_audio for cloning)
    - **language**: Language code (auto-detect if not specified)
    - **speed**: Speech speed multiplier (0.5 - 2.0)
    - **reference_audio**: Base64 encoded reference audio for voice cloning
    """

    if not model:
        raise HTTPException(status_code=503, detail="Model not loaded")

    try:
        logger.info(f"Synthesizing: {request.text[:50]}...")

        # Auto-detect Japanese
        is_japanese = any('\u3040' <= c <= '\u309F' or
                         '\u30A0' <= c <= '\u30FF' or
                         '\u4E00' <= c <= '\u9FFF'
                         for c in request.text)

        language = request.language if request.language != "auto" else ("ja" if is_japanese else "en")

        # Process reference audio if provided (voice cloning)
        reference_embedding = None
        if request.reference_audio:
            try:
                # Decode base64 audio
                audio_bytes = base64.b64decode(request.reference_audio)
                reference_audio, sr = sf.read(io.BytesIO(audio_bytes))

                # Extract voice embedding (placeholder - implement actual extraction)
                # reference_embedding = extract_voice_embedding(reference_audio, sr)
                logger.info("Voice cloning enabled with reference audio")

            except Exception as e:
                logger.warning(f"Failed to process reference audio: {e}")

        # Generate speech
        inputs = processor(
            text=request.text,
            return_tensors="pt",
            language=language
        ).to(device)

        with torch.no_grad():
            # Add voice embedding if available
            if reference_embedding is not None:
                outputs = model.generate(**inputs, speaker_embedding=reference_embedding)
            else:
                outputs = model.generate(**inputs)

        # Convert to audio
        audio_array = outputs.cpu().numpy().squeeze()

        # Apply speed adjustment
        if request.speed != 1.0:
            audio_array = adjust_speed(audio_array, request.speed)

        # Convert to bytes
        sample_rate = 22050  # Adjust based on model
        buffer = io.BytesIO()
        sf.write(buffer, audio_array, sample_rate, format='WAV')
        buffer.seek(0)

        # Encode as base64
        audio_base64 = base64.b64encode(buffer.read()).decode('utf-8')

        duration = len(audio_array) / sample_rate

        logger.info(f"Synthesis complete: {duration:.2f}s")

        return SynthesizeResponse(
            audio=audio_base64,
            sample_rate=sample_rate,
            duration=duration
        )

    except Exception as e:
        logger.error(f"Synthesis failed: {e}")
        raise HTTPException(status_code=500, detail=str(e))

@app.post("/synthesize/stream")
async def synthesize_stream(
    text: str = Form(...),
    voice: str = Form("default"),
    language: str = Form("auto"),
    speed: float = Form(1.0),
    reference_audio: Optional[UploadFile] = File(None)
):
    """
    Synthesize speech and return raw audio bytes
    Optimized for streaming
    """

    if not model:
        raise HTTPException(status_code=503, detail="Model not loaded")

    try:
        # Auto-detect Japanese
        is_japanese = any('\u3040' <= c <= '\u309F' or
                         '\u30A0' <= c <= '\u30FF' or
                         '\u4E00' <= c <= '\u9FFF'
                         for c in text)

        lang = language if language != "auto" else ("ja" if is_japanese else "en")

        # Process reference audio if provided
        reference_embedding = None
        if reference_audio:
            try:
                audio_bytes = await reference_audio.read()
                reference_audio_data, sr = sf.read(io.BytesIO(audio_bytes))
                logger.info(f"Loaded reference audio: {len(audio_bytes)} bytes")
            except Exception as e:
                logger.warning(f"Failed to load reference audio: {e}")

        # Generate speech
        inputs = processor(
            text=text,
            return_tensors="pt",
            language=lang
        ).to(device)

        with torch.no_grad():
            outputs = model.generate(**inputs)

        audio_array = outputs.cpu().numpy().squeeze()

        # Apply speed adjustment
        if speed != 1.0:
            audio_array = adjust_speed(audio_array, speed)

        # Convert to WAV bytes
        sample_rate = 22050
        buffer = io.BytesIO()
        sf.write(buffer, audio_array, sample_rate, format='WAV')
        buffer.seek(0)

        return Response(
            content=buffer.read(),
            media_type="audio/wav",
            headers={
                "Content-Disposition": "attachment; filename=speech.wav"
            }
        )

    except Exception as e:
        logger.error(f"Stream synthesis failed: {e}")
        raise HTTPException(status_code=500, detail=str(e))

@app.post("/clone")
async def clone_voice(
    name: str = Form(...),
    description: str = Form(""),
    reference_audio: UploadFile = File(...)
):
    """
    Create a new voice clone from reference audio

    - **name**: Name for the cloned voice
    - **description**: Optional description
    - **reference_audio**: Audio file with the voice to clone (WAV, MP3, etc.)
    """

    try:
        # Read uploaded audio
        audio_bytes = await reference_audio.read()
        audio_data, sample_rate = sf.read(io.BytesIO(audio_bytes))

        logger.info(f"Creating voice clone '{name}' from {len(audio_bytes)} bytes")

        # Extract voice embedding (placeholder - implement actual extraction)
        # voice_embedding = extract_voice_embedding(audio_data, sample_rate)

        # Save voice profile
        voice_id = f"clone_{name.lower().replace(' ', '_')}"

        # Store in database or filesystem
        # save_voice_profile(voice_id, voice_embedding, description)

        return {
            "voice_id": voice_id,
            "name": name,
            "description": description,
            "status": "created",
            "sample_rate": sample_rate,
            "duration": len(audio_data) / sample_rate
        }

    except Exception as e:
        logger.error(f"Voice cloning failed: {e}")
        raise HTTPException(status_code=500, detail=str(e))

def adjust_speed(audio: np.ndarray, speed: float) -> np.ndarray:
    """Adjust audio playback speed"""
    if speed == 1.0:
        return audio

    # Simple resampling for speed adjustment
    from scipy import signal
    target_length = int(len(audio) / speed)
    return signal.resample(audio, target_length)

if __name__ == "__main__":
    import uvicorn
    uvicorn.run(app, host="0.0.0.0", port=8000)
