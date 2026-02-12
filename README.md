# Peppemon

Real-time TUI system performance monitor for Linux.

## Features

- **CPU** — Per-core bar chart with color-coded usage, temperature, and frequency
- **Memory** — RAM and swap gauges with sparkline history
- **Network** — RX/TX rates with sparkline graphs
- **Disk I/O** — Read/write rates with sparkline graphs
- **Processes** — Sortable, filterable process list with scroll
- **System Info** — Kernel, hostname, uptime, load averages, governor, and more
- **Three Views** — Overview, full Processes, and CPU Detail tabs

## Install

### Quick (Ubuntu)

```bash
git clone https://github.com/Frescofree/peppemon.git
cd peppemon
./install.sh
```

### Manual

Requires Rust toolchain (`cargo`).

```bash
cargo build --release
sudo install -m 755 target/release/peppemon /usr/local/bin/peppemon
```

## Keybindings

| Key | Action |
|-----|--------|
| `Tab` | Cycle tabs (Overview / Processes / CPU Detail) |
| `q` | Quit |
| `?` | Toggle help overlay |
| `/` | Filter processes (type to search, Esc to clear) |
| `c` | Sort processes by CPU |
| `m` | Sort processes by Memory |
| `p` | Sort processes by PID |
| `Up`/`Down` | Scroll process list |
| `Esc` | Close filter/help, or quit |

## Troubleshooting

The installer runs pre-flight checks and shows specific errors, but here are the common issues:

| Problem | Fix |
|---------|-----|
| `apt-get not found` | You're not on Ubuntu/Debian. Install `gcc`, `make`, `pkg-config` with your distro's package manager, then run `cargo build --release` manually |
| `sudo not found` | Run `apt-get install sudo` as root, or ask your sysadmin |
| `Failed to download rustup` | Check internet connection. Behind a proxy? Set `HTTPS_PROXY=http://proxy:port` before running |
| `Build failed` | Usually low disk space — need ~500MB free. Try `df -h .` to check, then `cargo clean && cargo build --release` |
| `cargo: command not found` after install | Rust was installed but your shell doesn't see it yet. Run `source ~/.cargo/env` or open a new terminal |
| Peppemon runs but no CPU temperature | Your CPU sensor may use a different hwmon name. Check `cat /sys/class/hwmon/*/name` to see what's available |
| Blank/garbled display | Terminal too small — resize to at least 80x24. Or try a different terminal emulator |

### Manual install (any Linux distro)

If the installer doesn't work for your distro:

```bash
# 1. Install Rust: https://rustup.rs
# 2. Install your distro's C compiler + pkg-config
# 3. Then:
cargo build --release
sudo install -m 755 target/release/peppemon /usr/local/bin/peppemon
```

## License

MIT
