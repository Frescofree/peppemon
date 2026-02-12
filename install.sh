#!/usr/bin/env bash
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

ok()   { echo -e "  ${GREEN}✓${NC} $1"; }
warn() { echo -e "  ${YELLOW}!${NC} $1"; }
fail() { echo -e "  ${RED}✗${NC} $1"; }

echo "=== Peppemon Installer ==="
echo ""

# ── Pre-flight checks ──────────────────────────────────────────────────────

echo "[0/4] Running pre-flight checks..."

ERRORS=0

# Must be Linux
if [[ "$(uname -s)" != "Linux" ]]; then
    fail "Peppemon only supports Linux (detected: $(uname -s))"
    exit 1
fi
ok "Linux detected"

# Check for apt (Debian/Ubuntu)
if ! command -v apt-get &>/dev/null; then
    fail "apt-get not found — this installer requires Ubuntu or Debian"
    echo "       For other distros, install build-essential + pkg-config manually,"
    echo "       then run: cargo build --release && sudo install -m 755 target/release/peppemon /usr/local/bin/"
    exit 1
fi
ok "apt-get available"

# Check sudo
if ! command -v sudo &>/dev/null; then
    fail "sudo not found — needed to install packages and the binary"
    echo "       Run: apt-get install sudo   (as root)"
    exit 1
fi
if ! sudo -n true 2>/dev/null; then
    warn "sudo will prompt for your password during install"
else
    ok "sudo access confirmed"
fi

# Check curl (needed for rustup)
if ! command -v curl &>/dev/null; then
    warn "curl not found — will install it with apt"
else
    ok "curl available"
fi

# Check git (they likely used it to clone, but just in case)
if ! command -v git &>/dev/null; then
    warn "git not found — will install it with apt"
fi

# Check disk space (need ~500MB for Rust toolchain + build)
AVAIL_MB=$(df --output=avail -m "$(pwd)" | tail -1 | tr -d ' ')
if [[ "$AVAIL_MB" -lt 500 ]]; then
    fail "Low disk space: ${AVAIL_MB}MB available, need at least 500MB"
    echo "       Free up space and try again"
    exit 1
fi
ok "Disk space OK (${AVAIL_MB}MB available)"

# Check internet (quick DNS check)
if ! timeout 5 bash -c 'echo >/dev/tcp/github.com/443' 2>/dev/null; then
    warn "Cannot reach github.com — Rust install may fail without internet"
else
    ok "Internet connectivity OK"
fi

echo ""

# ── Step 1: System dependencies ────────────────────────────────────────────

echo "[1/4] Installing build dependencies..."
if ! sudo apt-get update -qq 2>/tmp/peppemon_apt_err; then
    fail "apt-get update failed"
    cat /tmp/peppemon_apt_err
    echo ""
    echo "  Troubleshooting:"
    echo "    - Check your internet connection"
    echo "    - Try: sudo apt-get update   (manually to see full errors)"
    exit 1
fi
if ! sudo apt-get install -y -qq build-essential pkg-config curl git 2>/tmp/peppemon_apt_err; then
    fail "apt-get install failed"
    cat /tmp/peppemon_apt_err
    echo ""
    echo "  Troubleshooting:"
    echo "    - You may have held/broken packages: sudo apt --fix-broken install"
    echo "    - Try manually: sudo apt-get install build-essential pkg-config curl git"
    exit 1
fi
ok "Build dependencies installed"

# ── Step 2: Rust toolchain ─────────────────────────────────────────────────

if ! command -v cargo &>/dev/null; then
    # Also check if cargo exists but isn't in PATH yet
    if [[ -f "$HOME/.cargo/env" ]]; then
        # shellcheck source=/dev/null
        source "$HOME/.cargo/env"
    fi
fi

if command -v cargo &>/dev/null; then
    echo "[2/4] Rust already installed ($(cargo --version)), skipping."
else
    echo "[2/4] Installing Rust toolchain..."
    RUSTUP_INIT=$(mktemp)
    if ! curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o "$RUSTUP_INIT"; then
        fail "Failed to download rustup"
        echo ""
        echo "  Troubleshooting:"
        echo "    - Check your internet connection"
        echo "    - If behind a proxy, set HTTPS_PROXY env variable"
        echo "    - Try downloading manually: https://sh.rustup.rs"
        rm -f "$RUSTUP_INIT"
        exit 1
    fi
    if ! sh "$RUSTUP_INIT" -y --default-toolchain stable; then
        fail "Rust installation failed"
        echo ""
        echo "  Troubleshooting:"
        echo "    - Check disk space (need ~300MB for toolchain)"
        echo "    - Try manual install: https://rustup.rs"
        rm -f "$RUSTUP_INIT"
        exit 1
    fi
    rm -f "$RUSTUP_INIT"
    # shellcheck source=/dev/null
    source "$HOME/.cargo/env"
    ok "Rust $(cargo --version) installed"
fi

# ── Step 3: Build ──────────────────────────────────────────────────────────

echo "[3/4] Building peppemon (release)..."
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

if ! cargo build --release 2>/tmp/peppemon_build_err; then
    fail "Build failed"
    echo ""
    tail -20 /tmp/peppemon_build_err
    echo ""
    echo "  Troubleshooting:"
    echo "    - Check disk space: df -h ."
    echo "    - Check Rust version: rustup update stable"
    echo "    - Clean and retry: cargo clean && cargo build --release"
    exit 1
fi
ok "Build successful"

# ── Step 4: Install binary ─────────────────────────────────────────────────

echo "[4/4] Installing binary..."
if ! sudo install -m 755 target/release/peppemon /usr/local/bin/peppemon; then
    fail "Could not install to /usr/local/bin/"
    echo ""
    echo "  Troubleshooting:"
    echo "    - Check permissions: ls -la /usr/local/bin/"
    echo "    - Alternative: copy manually to ~/bin/ and add to PATH"
    exit 1
fi
ok "Installed to /usr/local/bin/peppemon"

# Desktop entry (non-critical, don't fail on this)
DESKTOP_DIR="$HOME/.local/share/applications"
mkdir -p "$DESKTOP_DIR" 2>/dev/null || true
cat > "$DESKTOP_DIR/peppemon.desktop" <<'DESKTOP' 2>/dev/null || true
[Desktop Entry]
Name=Peppemon
Comment=Real-time system performance monitor
Exec=peppemon
Terminal=true
Type=Application
Categories=System;Monitor;
DESKTOP

# ── Done ───────────────────────────────────────────────────────────────────

echo ""
echo -e "${GREEN}=== Installation complete! ===${NC}"
echo ""
echo "  Run:  peppemon"
echo "  Help: peppemon then press ?"
echo ""
