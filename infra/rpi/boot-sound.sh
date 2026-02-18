#!/usr/bin/env bash
set -euo pipefail

# boot-sound.sh — Play boot chime when network is ready
#
# Called by chatweb-boot-sound.service after network-online.target.
# Uses sox to synthesize an ascending 3-note chime (C5-E5-G5).
# No WAV file needed — generated on the fly.
#
# Manual test:  sudo bash /opt/chatweb/boot-sound.sh
# Volume test:  amixer sset 'Headphone' 80%

CHIME_WAV="/tmp/chatweb-chime.wav"

# Ensure audio output goes to 3.5mm jack (not HDMI)
# numid=3: 0=auto, 1=analog(3.5mm), 2=HDMI
amixer cset numid=3 1 &>/dev/null || true

# Set volume (0-100%)
amixer sset 'Headphone' 80% &>/dev/null || \
amixer sset 'PCM' 80% &>/dev/null || true

# Generate ascending chime: C5 → E5 → G5 (major triad, bright & pleasant)
# Each note with a gentle fade-in/out
if command -v sox &>/dev/null; then
    sox -n "$CHIME_WAV" \
        synth 0.15 sine 523.25 vol 0.6 fade t 0.02 0.15 0.04 \
        : synth 0.15 sine 659.25 vol 0.6 fade t 0.02 0.15 0.04 \
        : synth 0.35 sine 783.99 vol 0.6 fade t 0.02 0.35 0.10 \
        gain -3
    aplay -q "$CHIME_WAV" 2>/dev/null
    rm -f "$CHIME_WAV"
else
    # Fallback: simple beep via speaker-test (less pleasant but works without sox)
    speaker-test -t sine -f 880 -l 1 -p 1 &>/dev/null &
    sleep 0.3
    kill $! 2>/dev/null || true
fi

logger -t chatweb "Boot chime played — network ready"
