#!/usr/bin/env bash
set -euo pipefail

# install-service.sh — Initial service setup on Raspberry Pi
#
# Run this ONCE on a fresh Pi to set up the chatweb service.
# Can be piped via SSH:
#   ssh -i ~/.ssh/id_ed25519_rpi yuki@chatweb-pi.local 'bash -s' < infra/rpi/install-service.sh

echo "=== chatweb Service Installation ==="

# Must be root or have sudo
if [ "$(id -u)" -ne 0 ]; then
    echo "Re-running with sudo..."
    exec sudo bash "$0" "$@"
fi

# --- 0. Install audio dependencies ---
echo "--- Installing audio packages ---"
apt-get update -qq
apt-get install -y -qq sox alsa-utils
echo "Installed sox + alsa-utils"

# --- 1. Create service user ---
if id -u chatweb &>/dev/null; then
    echo "Service user 'chatweb' already exists"
else
    useradd --system --no-create-home --shell /usr/sbin/nologin chatweb
    echo "Created service user 'chatweb'"
fi

# --- 2. Create service directory ---
mkdir -p /opt/chatweb
chown chatweb:chatweb /opt/chatweb
chmod 750 /opt/chatweb
echo "Created /opt/chatweb"

# --- 3. Create workspace directory (for file-backend sessions) ---
mkdir -p /opt/chatweb/.nanobot/workspace
mkdir -p /opt/chatweb/.nanobot/workspace/memory
chown -R chatweb:chatweb /opt/chatweb/.nanobot
echo "Created workspace at /opt/chatweb/.nanobot/workspace"

# --- 4. Create .env placeholder ---
if [ ! -f /opt/chatweb/.env ]; then
    cat > /opt/chatweb/.env << 'ENVEOF'
# chatweb environment — edit with actual API keys
RUST_LOG=info
# ANTHROPIC_API_KEY=sk-ant-...
# OPENAI_API_KEY=sk-...
# OPENROUTER_API_KEY=sk-or-...
ENVEOF
    chown chatweb:chatweb /opt/chatweb/.env
    chmod 600 /opt/chatweb/.env
    echo "Created /opt/chatweb/.env (placeholder — add API keys!)"
else
    echo "/opt/chatweb/.env already exists"
fi

# --- 5. Install systemd service ---
cat > /etc/systemd/system/chatweb.service << 'SERVICEEOF'
[Unit]
Description=chatweb AI Assistant (Raspberry Pi)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=chatweb
Group=chatweb
WorkingDirectory=/opt/chatweb
ExecStart=/opt/chatweb/chatweb gateway --http --http-port 3000
EnvironmentFile=/opt/chatweb/.env
Restart=always
RestartSec=5

# Resource limits (RPi4 4GB)
MemoryMax=1G
MemoryHigh=768M

# Security hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/opt/chatweb
PrivateTmp=true
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectControlGroups=true
RestrictSUIDSGID=true
RestrictNamespaces=true

# Logging
StandardOutput=journal
StandardError=journal
SyslogIdentifier=chatweb

[Install]
WantedBy=multi-user.target
SERVICEEOF

echo "Installed chatweb systemd service"

# --- 6. Install boot sound ---
echo "--- Installing boot sound service ---"

# Copy boot-sound script
cp /dev/stdin /opt/chatweb/boot-sound.sh << 'SOUNDEOF'
#!/usr/bin/env bash
set -euo pipefail
CHIME_WAV="/tmp/chatweb-chime.wav"
amixer cset numid=3 1 &>/dev/null || true
amixer sset 'Headphone' 80% &>/dev/null || amixer sset 'PCM' 80% &>/dev/null || true
if command -v sox &>/dev/null; then
    sox -n "$CHIME_WAV" \
        synth 0.15 sine 523.25 vol 0.6 fade t 0.02 0.15 0.04 \
        : synth 0.15 sine 659.25 vol 0.6 fade t 0.02 0.15 0.04 \
        : synth 0.35 sine 783.99 vol 0.6 fade t 0.02 0.35 0.10 \
        gain -3
    aplay -q "$CHIME_WAV" 2>/dev/null
    rm -f "$CHIME_WAV"
else
    speaker-test -t sine -f 880 -l 1 -p 1 &>/dev/null &
    sleep 0.3; kill $! 2>/dev/null || true
fi
logger -t chatweb "Boot chime played — network ready"
SOUNDEOF
chmod +x /opt/chatweb/boot-sound.sh

# Install boot-sound systemd service
cat > /etc/systemd/system/chatweb-boot-sound.service << 'BOOTSNDEOF'
[Unit]
Description=chatweb boot chime (plays when network is ready)
After=network-online.target sound.target
Wants=network-online.target

[Service]
Type=oneshot
ExecStart=/opt/chatweb/boot-sound.sh
RemainAfterExit=false

# Audio needs access to /dev/snd
SupplementaryGroups=audio

[Install]
WantedBy=multi-user.target
BOOTSNDEOF

echo "Installed boot-sound service"

# --- 7. Enable audio output ---
echo "--- Enabling audio ---"
# Ensure analog audio is enabled in boot config
BOOT_CONFIG="/boot/firmware/config.txt"
if [ -f "$BOOT_CONFIG" ]; then
    if ! grep -q "^dtparam=audio=on" "$BOOT_CONFIG"; then
        echo "dtparam=audio=on" >> "$BOOT_CONFIG"
        echo "Enabled audio in config.txt"
    else
        echo "Audio already enabled in config.txt"
    fi
fi

# --- 8. Enable and reload ---
systemctl daemon-reload
systemctl enable chatweb
systemctl enable chatweb-boot-sound
echo "Services enabled (chatweb + boot-sound)"

# --- 9. Summary ---
echo ""
echo "=== Installation Complete ==="
echo ""
echo "Service directory: /opt/chatweb"
echo "Config:            /opt/chatweb/.env"
echo "Workspace:         /opt/chatweb/.nanobot/workspace"
echo "Logs:              journalctl -u chatweb -f"
echo ""
echo "Next steps:"
echo "  1. Edit /opt/chatweb/.env — add your API keys"
echo "  2. From your Mac, run: ./infra/rpi/deploy-rpi.sh"
echo "  3. Access: http://chatweb-pi.local:3000"
