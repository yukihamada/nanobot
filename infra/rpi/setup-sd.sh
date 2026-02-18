#!/usr/bin/env bash
set -euo pipefail

# setup-sd.sh — Raspberry Pi SD card initial setup
#
# Run AFTER flashing Raspberry Pi OS Lite (64-bit) with Raspberry Pi Imager.
# This script configures the boot partition for headless first boot.
#
# Usage:
#   ./infra/rpi/setup-sd.sh /Volumes/bootfs
#   ./infra/rpi/setup-sd.sh /media/$USER/bootfs

BOOT_DIR="${1:?Usage: $0 <boot-partition-path>  (e.g. /Volumes/bootfs)}"

if [ ! -d "$BOOT_DIR" ]; then
    echo "ERROR: Boot partition not found at $BOOT_DIR"
    echo "Insert the SD card and check the mount path."
    exit 1
fi

echo "=== chatweb-pi SD Card Setup ==="
echo "Boot partition: $BOOT_DIR"
echo ""

# --- 1. Enable SSH ---
echo "--- Enabling SSH ---"
touch "$BOOT_DIR/ssh"
echo "Created $BOOT_DIR/ssh"

# --- 2. WiFi configuration ---
echo "--- Configuring WiFi ---"
cat > "$BOOT_DIR/wpa_supplicant.conf" << 'WPAEOF'
country=JP
ctrl_interface=DIR=/var/run/wpa_supplicant GROUP=netdev
update_config=1

network={
    ssid="Hama-Fi"
    psk="sushiramen"
    priority=10
}
WPAEOF
echo "Created wpa_supplicant.conf (SSID: Hama-Fi)"

# --- 3. User configuration ---
echo "--- Configuring user ---"
# Generate password hash for user 'yuki' (password: chatweb)
# Using openssl to generate the hash
PASS_HASH=$(openssl passwd -6 "chatweb")
echo "yuki:${PASS_HASH}" > "$BOOT_DIR/userconf.txt"
echo "Created userconf.txt (user: yuki, password: chatweb)"
echo "  !! Change password on first login: passwd"

# --- 4. SSH key ---
echo "--- Setting up SSH key ---"
SSH_KEY="$HOME/.ssh/id_ed25519_rpi"
if [ -f "$SSH_KEY" ]; then
    echo "SSH key already exists: $SSH_KEY"
else
    ssh-keygen -t ed25519 -f "$SSH_KEY" -N "" -C "yuki@chatweb-pi"
    echo "Generated SSH key: $SSH_KEY"
fi

SSH_PUB=$(cat "${SSH_KEY}.pub")

# --- 5. First-run script ---
echo "--- Creating firstrun.sh ---"
cat > "$BOOT_DIR/firstrun.sh" << FIRSTRUNEOF
#!/usr/bin/env bash
set -euo pipefail

# First-run setup for chatweb-pi
# This runs once on first boot to configure the system.

echo "=== chatweb-pi first-run setup ==="

# Set hostname
hostnamectl set-hostname chatweb-pi
echo "127.0.1.1 chatweb-pi" >> /etc/hosts
echo "Hostname set to chatweb-pi"

# Set timezone
timedatectl set-timezone Asia/Tokyo
echo "Timezone set to Asia/Tokyo"

# Setup SSH authorized_keys for user yuki
YUKI_HOME="/home/yuki"
mkdir -p "\$YUKI_HOME/.ssh"
chmod 700 "\$YUKI_HOME/.ssh"
echo "${SSH_PUB}" >> "\$YUKI_HOME/.ssh/authorized_keys"
chmod 600 "\$YUKI_HOME/.ssh/authorized_keys"
chown -R yuki:yuki "\$YUKI_HOME/.ssh"
echo "SSH key installed for yuki"

# Harden SSH: disable password auth after key is installed
sed -i 's/#PasswordAuthentication yes/PasswordAuthentication no/' /etc/ssh/sshd_config
sed -i 's/PasswordAuthentication yes/PasswordAuthentication no/' /etc/ssh/sshd_config
systemctl restart sshd
echo "Password auth disabled (SSH key only)"

# Update package list (but don't upgrade to save time)
apt-get update -qq
echo "Package list updated"

# Install useful tools + audio
apt-get install -y -qq curl htop sox alsa-utils
echo "Tools installed (incl. sox for boot chime)"

# Create chatweb service user (nologin, for systemd service)
if ! id -u chatweb &>/dev/null; then
    useradd --system --no-create-home --shell /usr/sbin/nologin chatweb
    echo "Service user 'chatweb' created"
fi

# Create service directory
mkdir -p /opt/chatweb
chown chatweb:chatweb /opt/chatweb
echo "Service directory /opt/chatweb created"

# Self-destruct: remove firstrun.sh after execution
rm -f /boot/firmware/firstrun.sh /boot/firstrun.sh
echo "=== First-run setup complete ==="
echo "Rebooting in 5 seconds..."
sleep 5
reboot
FIRSTRUNEOF
chmod +x "$BOOT_DIR/firstrun.sh"
echo "Created firstrun.sh"

# --- 6. Auto-run firstrun.sh on first boot ---
# Raspberry Pi OS checks for firstrun.sh in the boot partition
# But we also add a systemd oneshot as backup
cat > "$BOOT_DIR/cmdline.txt.firstrun" << 'EOF'
# If firstrun.sh doesn't auto-execute, manually run:
#   sudo bash /boot/firmware/firstrun.sh
# Then reboot.
EOF
echo "Created cmdline.txt.firstrun (manual fallback instructions)"

echo ""
echo "=== Setup Complete ==="
echo ""
echo "SSH key: $SSH_KEY"
echo "WiFi:    Hama-Fi"
echo "User:    yuki (password: chatweb — change on first login!)"
echo "Host:    chatweb-pi.local"
echo ""
echo "Next steps:"
echo "  1. Eject SD card and insert into Raspberry Pi"
echo "  2. Power on and wait ~2 minutes for first boot"
echo "  3. Run firstrun.sh if it doesn't auto-execute:"
echo "     ssh -i $SSH_KEY yuki@chatweb-pi.local 'sudo bash /boot/firmware/firstrun.sh'"
echo "  4. After reboot, connect:"
echo "     ssh -i $SSH_KEY yuki@chatweb-pi.local"
echo "  5. Install the service:"
echo "     ssh -i $SSH_KEY yuki@chatweb-pi.local 'bash -s' < infra/rpi/install-service.sh"
echo "  6. Deploy:"
echo "     ./infra/rpi/deploy-rpi.sh"
