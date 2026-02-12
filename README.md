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
git clone https://github.com/peppe/peppemon.git
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

## License

MIT
