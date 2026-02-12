#!/usr/bin/env bash
set -euo pipefail

echo "=== Peppemon Installer ==="
echo ""

# 1. Install system dependencies
echo "[1/4] Installing build dependencies..."
sudo apt-get update -qq
sudo apt-get install -y -qq build-essential pkg-config

# 2. Install Rust (if not present)
if ! command -v cargo &>/dev/null; then
    echo "[2/4] Installing Rust toolchain..."
    RUSTUP_INIT=$(mktemp)
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o "$RUSTUP_INIT"
    sh "$RUSTUP_INIT" -y --default-toolchain stable
    rm -f "$RUSTUP_INIT"
    # shellcheck source=/dev/null
    source "$HOME/.cargo/env"
else
    echo "[2/4] Rust already installed, skipping."
fi

# 3. Build
echo "[3/4] Building peppemon (release)..."
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"
cargo build --release

# 4. Install
echo "[4/4] Installing binary..."
sudo install -m 755 target/release/peppemon /usr/local/bin/peppemon

# Optional: desktop entry
DESKTOP_DIR="$HOME/.local/share/applications"
mkdir -p "$DESKTOP_DIR"
cat > "$DESKTOP_DIR/peppemon.desktop" <<'DESKTOP'
[Desktop Entry]
Name=Peppemon
Comment=Real-time system performance monitor
Exec=peppemon
Terminal=true
Type=Application
Categories=System;Monitor;
DESKTOP

echo ""
echo "Done! Run 'peppemon' to start."
