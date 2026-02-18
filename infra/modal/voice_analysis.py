"""
Voice Quality Analysis on Modal — Analyze recorded voice samples and return quality scores.

Deploy:  modal deploy voice_analysis.py
Test:    modal run voice_analysis.py
Logs:    modal app logs voice-analysis

Exposes:
  POST /analyze   — Accept audio (base64), return quality scores + metrics
                    Body: { audio_base64 }
                    Returns: JSON with scores, analysis, voice_type, language_detected
  GET  /health    — Health check
"""
import io
import modal

# ---------------------------------------------------------------------------
# Modal image: librosa + parselmouth (Praat) + audio processing
# ---------------------------------------------------------------------------
analysis_image = (
    modal.Image.debian_slim(python_version="3.11")
    .apt_install(
        "ffmpeg", "libsndfile1",
    )
    .pip_install(
        "numpy",
        "scipy",
        "librosa>=0.10",
        "soundfile",
        "pydub",
        "praat-parselmouth>=0.4",
        "fastapi",
        "uvicorn",
    )
)

app = modal.App("voice-analysis", image=analysis_image)


# ---------------------------------------------------------------------------
# Voice type classification based on pitch
# ---------------------------------------------------------------------------
VOICE_TYPE_RANGES = {
    # Female voice types (Hz ranges for fundamental frequency)
    "soprano":        (250, 1050),
    "mezzo-soprano":  (200, 700),
    "contralto":      (150, 500),
    # Male voice types
    "tenor":          (130, 500),
    "baritone":       (100, 400),
    "bass":           (65, 330),
}


def classify_voice_type(pitch_mean_hz: float, pitch_range: tuple) -> str:
    """Classify voice type based on mean pitch and range."""
    if pitch_mean_hz <= 0:
        return "unknown"

    # Gender estimation from pitch
    if pitch_mean_hz > 165:
        # Likely female
        if pitch_mean_hz > 250:
            return "soprano"
        elif pitch_mean_hz > 200:
            return "mezzo-soprano"
        else:
            return "contralto"
    else:
        # Likely male
        if pitch_mean_hz > 140:
            return "tenor"
        elif pitch_mean_hz > 100:
            return "baritone"
        else:
            return "bass"


def detect_language(audio_array, sr: int) -> str:
    """Simple language detection based on spectral characteristics.
    In production, use a proper language detection model.
    For now, default to 'ja' since our primary users are Japanese.
    """
    # Heuristic: Japanese speech tends to have a more concentrated spectral energy
    # in the 300-3000 Hz range with specific formant patterns.
    # This is a simplified heuristic — a real system would use Whisper or similar.
    return "ja"


# ---------------------------------------------------------------------------
# Core analysis functions
# ---------------------------------------------------------------------------

def compute_snr(audio, sr: int) -> float:
    """Compute Signal-to-Noise Ratio in dB."""
    import numpy as np

    # Simple energy-based SNR estimation
    # Split into frames, classify as speech or silence based on energy
    frame_length = int(0.025 * sr)  # 25ms frames
    hop_length = int(0.010 * sr)    # 10ms hop

    # Compute frame energies
    frames = []
    for i in range(0, len(audio) - frame_length, hop_length):
        frame = audio[i:i + frame_length]
        energy = np.sum(frame ** 2) / frame_length
        frames.append(energy)

    if not frames:
        return 0.0

    frames = np.array(frames)
    # Use energy threshold to separate speech from noise
    threshold = np.percentile(frames[frames > 0], 15) if np.any(frames > 0) else 0
    speech_energy = np.mean(frames[frames > threshold]) if np.any(frames > threshold) else 1e-10
    noise_energy = np.mean(frames[frames <= threshold]) if np.any(frames <= threshold) else 1e-10

    if noise_energy <= 0:
        noise_energy = 1e-10

    snr = 10 * np.log10(speech_energy / noise_energy)
    return float(np.clip(snr, 0, 60))


def compute_spectral_flatness_mean(audio, sr: int) -> float:
    """Compute mean spectral flatness (0=tonal, 1=noisy)."""
    import librosa
    import numpy as np

    flatness = librosa.feature.spectral_flatness(y=audio, n_fft=2048, hop_length=512)
    return float(np.mean(flatness))


def compute_spectral_centroid_mean(audio, sr: int) -> float:
    """Compute mean spectral centroid in Hz."""
    import librosa
    import numpy as np

    centroid = librosa.feature.spectral_centroid(y=audio, sr=sr, n_fft=2048, hop_length=512)
    return float(np.mean(centroid))


def compute_harmonic_ratio(audio, sr: int) -> float:
    """Compute harmonic-to-noise ratio using librosa's harmonic/percussive separation."""
    import librosa
    import numpy as np

    harmonic, percussive = librosa.effects.hpss(audio)
    h_energy = np.sum(harmonic ** 2)
    p_energy = np.sum(percussive ** 2)

    if p_energy <= 0:
        return 30.0  # Very harmonic

    hnr = 10 * np.log10(h_energy / p_energy)
    return float(np.clip(hnr, 0, 40))


def compute_dynamic_range(audio, sr: int) -> float:
    """Compute dynamic range in dB (difference between loud and quiet parts)."""
    import numpy as np

    frame_length = int(0.025 * sr)
    hop_length = int(0.010 * sr)

    energies = []
    for i in range(0, len(audio) - frame_length, hop_length):
        frame = audio[i:i + frame_length]
        rms = np.sqrt(np.mean(frame ** 2))
        if rms > 0:
            energies.append(20 * np.log10(rms))

    if not energies:
        return 0.0

    energies = np.array(energies)
    # Use percentiles to avoid outliers
    loud = np.percentile(energies, 95)
    quiet = np.percentile(energies, 5)
    return float(loud - quiet)


def analyze_with_parselmouth(audio_bytes: bytes) -> dict:
    """Run Praat-based analysis using parselmouth."""
    import parselmouth
    import numpy as np
    import tempfile
    import os

    # Write to temp file for parselmouth
    with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as f:
        f.write(audio_bytes)
        tmp_path = f.name

    try:
        snd = parselmouth.Sound(tmp_path)

        # Pitch analysis
        pitch = snd.to_pitch(time_step=0.01, pitch_floor=50, pitch_ceiling=600)
        pitch_values = pitch.selected_array["frequency"]
        pitch_values = pitch_values[pitch_values > 0]

        pitch_mean = float(np.mean(pitch_values)) if len(pitch_values) > 0 else 0
        pitch_min = float(np.min(pitch_values)) if len(pitch_values) > 0 else 0
        pitch_max = float(np.max(pitch_values)) if len(pitch_values) > 0 else 0
        pitch_std = float(np.std(pitch_values)) if len(pitch_values) > 0 else 0

        # Jitter and Shimmer
        point_process = parselmouth.praat.call(snd, "To PointProcess (periodic, cc)", 50, 600)

        jitter = 0.0
        shimmer = 0.0
        try:
            jitter = float(parselmouth.praat.call(
                point_process, "Get jitter (local)", 0.0, 0.0, 0.0001, 0.02, 1.3
            ))
        except Exception:
            pass

        try:
            shimmer = float(parselmouth.praat.call(
                [snd, point_process], "Get shimmer (local)", 0.0, 0.0, 0.0001, 0.02, 1.3, 1.6
            ))
        except Exception:
            pass

        # Speaking rate estimation (syllable-like events)
        intensity = snd.to_intensity(minimum_pitch=50)
        duration = snd.get_total_duration()

        # Estimate syllables from intensity peaks
        intensity_values = intensity.values[0]
        threshold = np.mean(intensity_values) - np.std(intensity_values)
        above = intensity_values > threshold
        # Count transitions from below to above threshold
        transitions = np.sum(np.diff(above.astype(int)) > 0)
        speaking_rate = transitions / duration if duration > 0 else 0

        return {
            "pitch_mean_hz": round(pitch_mean, 1),
            "pitch_min_hz": round(pitch_min, 1),
            "pitch_max_hz": round(pitch_max, 1),
            "pitch_std_hz": round(pitch_std, 2),
            "jitter_percent": round(jitter * 100, 2),
            "shimmer_percent": round(shimmer * 100, 2),
            "speaking_rate_syl_per_sec": round(speaking_rate, 1),
            "duration_sec": round(duration, 2),
        }

    finally:
        os.unlink(tmp_path)


def compute_scores(analysis: dict, snr: float, spectral_flatness: float,
                   spectral_centroid: float, harmonic_ratio: float,
                   dynamic_range: float) -> dict:
    """Compute quality scores (0-100) from raw metrics."""
    import numpy as np

    # --- Clarity score ---
    # Higher SNR = better clarity, lower spectral flatness = cleaner signal
    snr_score = np.clip(snr / 40 * 100, 0, 100)           # 40dB = perfect
    flatness_score = np.clip((1 - spectral_flatness * 10) * 100, 0, 100)
    clarity = int(np.clip(snr_score * 0.6 + flatness_score * 0.4, 30, 100))

    # --- Stability score ---
    # Lower jitter and shimmer = more stable voice
    jitter = analysis.get("jitter_percent", 2.0)
    shimmer = analysis.get("shimmer_percent", 5.0)
    jitter_score = np.clip((1 - jitter / 3.0) * 100, 0, 100)     # 3% = poor
    shimmer_score = np.clip((1 - shimmer / 10.0) * 100, 0, 100)   # 10% = poor
    stability = int(np.clip(jitter_score * 0.5 + shimmer_score * 0.5, 25, 100))

    # --- Warmth score ---
    # Lower spectral centroid + richer harmonics = warmer
    # Ideal centroid for warm voice: 1500-2500 Hz
    centroid_ideal = 2000
    centroid_diff = abs(spectral_centroid - centroid_ideal)
    centroid_score = np.clip((1 - centroid_diff / 3000) * 100, 0, 100)
    harmonic_score = np.clip(harmonic_ratio / 25 * 100, 0, 100)
    warmth = int(np.clip(centroid_score * 0.5 + harmonic_score * 0.5, 30, 100))

    # --- Expressiveness score ---
    # Wider F0 range + more dynamic range = more expressive
    pitch_range = analysis.get("pitch_max_hz", 200) - analysis.get("pitch_min_hz", 100)
    range_score = np.clip(pitch_range / 150 * 100, 0, 100)  # 150Hz range = very expressive
    dynamic_score = np.clip(dynamic_range / 40 * 100, 0, 100)  # 40dB dynamic range = excellent
    expressiveness = int(np.clip(range_score * 0.6 + dynamic_score * 0.4, 25, 100))

    # --- Listenability (MOS-like perceptual score) ---
    # Weighted combination of all factors
    listenability = int(np.clip(
        clarity * 0.30 +
        stability * 0.25 +
        warmth * 0.20 +
        expressiveness * 0.25,
        30, 100
    ))

    # --- Overall ---
    overall = int(np.clip(
        clarity * 0.25 +
        stability * 0.20 +
        warmth * 0.20 +
        expressiveness * 0.15 +
        listenability * 0.20,
        30, 100
    ))

    # Apply a gentle boost to make scores feel more rewarding
    # (shift distribution upward — nobody likes getting a 40)
    def boost(score):
        # Maps 30-100 to roughly 55-100
        return int(np.clip(55 + (score - 30) * (45.0 / 70.0), 55, 100))

    return {
        "clarity": boost(clarity),
        "stability": boost(stability),
        "warmth": boost(warmth),
        "expressiveness": boost(expressiveness),
        "listenability": boost(listenability),
        "overall": boost(overall),
    }


# ---------------------------------------------------------------------------
# Web endpoint — FastAPI ASGI app
# ---------------------------------------------------------------------------
@app.function(
    cpu=2.0,
    memory=2048,
    scaledown_window=120,
    image=analysis_image,
)
@modal.concurrent(max_inputs=4)
@modal.asgi_app()
def web():
    """FastAPI web endpoint for voice quality analysis."""
    import base64
    import json
    import numpy as np
    import soundfile as sf
    import librosa
    from fastapi import FastAPI, HTTPException, Response
    from fastapi.middleware.cors import CORSMiddleware
    from pydantic import BaseModel, Field
    from typing import Optional

    fastapi_app = FastAPI(title="Voice Quality Analysis", version="1.0")

    fastapi_app.add_middleware(
        CORSMiddleware,
        allow_origins=["*"],
        allow_methods=["*"],
        allow_headers=["*"],
    )

    class AnalyzeRequest(BaseModel):
        audio_base64: str
        sample_rate: Optional[int] = None

    def _decode_audio(audio_b64: str) -> tuple:
        """Decode base64 audio to numpy array + sample rate."""
        raw = base64.b64decode(audio_b64)

        # Try to load as any audio format
        try:
            buf = io.BytesIO(raw)
            from pydub import AudioSegment
            for fmt in ["webm", "ogg", "mp3", "wav", "m4a"]:
                try:
                    buf.seek(0)
                    seg = AudioSegment.from_file(buf, format=fmt)
                    # Convert to mono WAV
                    seg = seg.set_channels(1)
                    wav_buf = io.BytesIO()
                    seg.export(wav_buf, format="wav")
                    wav_buf.seek(0)
                    audio, sr = sf.read(wav_buf)
                    return audio.astype(np.float32), sr
                except Exception:
                    continue

            # Last resort: try raw
            buf.seek(0)
            seg = AudioSegment.from_file(buf)
            seg = seg.set_channels(1)
            wav_buf = io.BytesIO()
            seg.export(wav_buf, format="wav")
            wav_buf.seek(0)
            audio, sr = sf.read(wav_buf)
            return audio.astype(np.float32), sr

        except Exception as e:
            raise ValueError(f"Failed to decode audio: {e}")

    def _get_wav_bytes(audio: np.ndarray, sr: int) -> bytes:
        """Convert numpy audio to WAV bytes for parselmouth."""
        wav_buf = io.BytesIO()
        sf.write(wav_buf, audio, sr, format="WAV")
        return wav_buf.getvalue()

    @fastapi_app.post("/analyze")
    async def analyze(req: AnalyzeRequest):
        """Analyze voice quality from audio sample."""
        if not req.audio_base64:
            raise HTTPException(400, "audio_base64 is required")

        # Validate size (max 10MB base64)
        if len(req.audio_base64) > 10_000_000:
            raise HTTPException(400, "Audio sample too large (max 10MB)")

        try:
            # Decode audio
            audio, sr = _decode_audio(req.audio_base64)

            # Resample to 16kHz for consistent analysis
            if sr != 16000:
                audio = librosa.resample(audio, orig_sr=sr, target_sr=16000)
                sr = 16000

            # Ensure minimum duration (1 second)
            if len(audio) / sr < 0.5:
                raise HTTPException(400, "Audio too short (minimum 0.5 seconds)")

            # Trim silence
            audio_trimmed, _ = librosa.effects.trim(audio, top_db=30)
            if len(audio_trimmed) / sr < 0.3:
                audio_trimmed = audio  # Use original if trimmed is too short

            # Get WAV bytes for parselmouth
            wav_bytes = _get_wav_bytes(audio_trimmed, sr)

            # Run analyses in parallel-ish fashion
            praat_analysis = analyze_with_parselmouth(wav_bytes)
            snr = compute_snr(audio_trimmed, sr)
            spectral_flatness = compute_spectral_flatness_mean(audio_trimmed, sr)
            spectral_centroid = compute_spectral_centroid_mean(audio_trimmed, sr)
            harmonic_ratio = compute_harmonic_ratio(audio_trimmed, sr)
            dynamic_range = compute_dynamic_range(audio_trimmed, sr)

            # Compute scores
            scores = compute_scores(
                praat_analysis, snr, spectral_flatness,
                spectral_centroid, harmonic_ratio, dynamic_range
            )

            # Classify voice type
            voice_type = classify_voice_type(
                praat_analysis["pitch_mean_hz"],
                (praat_analysis["pitch_min_hz"], praat_analysis["pitch_max_hz"])
            )

            # Detect language
            language = detect_language(audio_trimmed, sr)

            # Build response
            analysis = {
                "pitch_mean_hz": praat_analysis["pitch_mean_hz"],
                "pitch_range_hz": [
                    praat_analysis["pitch_min_hz"],
                    praat_analysis["pitch_max_hz"]
                ],
                "snr_db": round(snr, 1),
                "jitter_percent": praat_analysis["jitter_percent"],
                "shimmer_percent": praat_analysis["shimmer_percent"],
                "speaking_rate_syl_per_sec": praat_analysis["speaking_rate_syl_per_sec"],
                "spectral_centroid_hz": round(spectral_centroid, 0),
                "harmonic_ratio_db": round(harmonic_ratio, 1),
                "dynamic_range_db": round(dynamic_range, 1),
                "duration_sec": praat_analysis["duration_sec"],
            }

            return {
                "scores": scores,
                "analysis": analysis,
                "voice_type": voice_type,
                "language_detected": language,
            }

        except ValueError as e:
            raise HTTPException(400, str(e))
        except HTTPException:
            raise
        except Exception as e:
            print(f"Analysis error: {e}")
            import traceback
            traceback.print_exc()
            raise HTTPException(500, f"Voice analysis failed: {str(e)}")

    @fastapi_app.get("/health")
    async def health():
        return {
            "status": "ok",
            "service": "voice-analysis",
            "version": "1.0",
            "features": ["clarity", "stability", "warmth", "expressiveness", "listenability"],
        }

    return fastapi_app


# ---------------------------------------------------------------------------
# CLI: quick test via `modal run voice_analysis.py`
# ---------------------------------------------------------------------------
@app.local_entrypoint()
def main():
    """Test the voice analysis deployment."""
    print("Voice Quality Analysis on Modal")
    print("Deploy with: modal deploy voice_analysis.py")
    print("Then test with:")
    print('  curl -X POST https://<your-url>/analyze \\')
    print('    -H "Content-Type: application/json" \\')
    print('    -d \'{"audio_base64": "<base64-encoded-audio>"}\'')
    print()
    print("Response format:")
    print('  { "scores": { "clarity": 85, "stability": 78, ... },')
    print('    "analysis": { "pitch_mean_hz": 165, ... },')
    print('    "voice_type": "mezzo-soprano",')
    print('    "language_detected": "ja" }')
