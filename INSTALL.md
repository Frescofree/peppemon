# Installing Peppemon on Ubuntu

Step-by-step guide for a vanilla Ubuntu system with no Rust toolchain installed.

## 1. Update your system

```bash
sudo apt update && sudo apt upgrade -y
```

## 2. Install build essentials

Rust needs a C linker and basic build tools:

```bash
sudo apt install -y build-essential pkg-config
```

## 3. Install Rust

The official installer from rustup.rs:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

When prompted, choose option **1** (default installation).

After it finishes, load Rust into your current shell:

```bash
source "$HOME/.cargo/env"
```

> **Tip:** This is also added to your `~/.bashrc` (or `~/.zshrc`) automatically,
> so future terminal sessions will have Rust available without this step.

## 4. Verify the installation

```bash
rustc --version
cargo --version
```

You should see version numbers for both. If you get "command not found", run
`source "$HOME/.cargo/env"` again or open a new terminal.

## 5. Clone the repository

If you don't have git installed:

```bash
sudo apt install -y git
```

Then clone and enter the directory:

```bash
git clone https://github.com/peppemon/peppemon.git
cd peppemon
```

## 6. Build

```bash
cargo build --release
```

The first build downloads and compiles all dependencies. This is normal and
only happens once; subsequent builds are much faster.

The compiled binary will be at `./target/release/peppemon`.

## 7. Run

```bash
./target/release/peppemon
```

For the best experience, use a terminal at least **80 columns x 24 rows** wide.
Resize your terminal if widgets look cramped.

Press `?` inside peppemon to see all keybindings.

## 8. Optional: install system-wide

```bash
sudo cp target/release/peppemon /usr/local/bin/
```

Now you can run `peppemon` from anywhere.

## Troubleshooting

### "linker `cc` not found" or "cannot find -lgcc"

You're missing build tools. Run:

```bash
sudo apt install -y build-essential
```

### "rustc: command not found" after installing Rust

Your shell doesn't have Cargo's bin directory in PATH yet:

```bash
source "$HOME/.cargo/env"
```

Or open a new terminal window.

### Terminal too small

Peppemon needs at least ~80x24. Check your terminal size:

```bash
tput cols; tput lines
```

Resize the window or reduce font size if needed.

### No CPU temperature readings

Temperature sensors require read access to `/sys/class/hwmon/` or
`/sys/class/thermal/`. This works on most bare-metal machines but may not
be available inside VMs or containers. Peppemon will still run; it just
won't display temperature data.

### Permission denied errors

If you see permission errors when reading system files, ensure you're running
as a regular user (not root). Peppemon reads from `/proc` and `/sys` which
are normally world-readable.

### Build takes a very long time

The first build compiles all dependencies from source. On a low-spec machine
this can take several minutes. Subsequent `cargo build` runs will be
incremental and much faster.
