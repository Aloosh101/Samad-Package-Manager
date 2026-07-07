# Installation

## Quick Install (via script)

```bash
curl -fsSL https://github.com/Aloosh101/Samad-Package-Manager/releases/latest/download/install.sh | sudo bash
```

This auto-detects your architecture and OS, downloads the correct binary for both `spm` (CLI) and `spmd` (daemon), and installs them to `/usr/local/bin/`.

### Options

```bash
# System-wide install to /usr/local/bin (default)
curl -fsSL https://github.com/Aloosh101/Samad-Package-Manager/releases/latest/download/install.sh | sudo bash

# User install (~/.local/bin)
curl -fsSL https://github.com/Aloosh101/Samad-Package-Manager/releases/latest/download/install.sh | bash -s -- --user
```

## Manual Install

Download the binaries for your architecture from the [releases page](https://github.com/Aloosh101/Samad-Package-Manager/releases):

```bash
# Example: x86_64 Linux
wget https://github.com/Aloosh101/Samad-Package-Manager/releases/latest/download/spm-x86_64-linux-gnu
wget https://github.com/Aloosh101/Samad-Package-Manager/releases/latest/download/spmd-x86_64-linux-gnu
chmod +x spm-x86_64-linux-gnu spmd-x86_64-linux-gnu
sudo mv spm-x86_64-linux-gnu /usr/local/bin/spm
sudo mv spmd-x86_64-linux-gnu /usr/local/bin/spmd
```

## Self-Update

If you already have a working installation, update with:

```bash
# Update both spm and spmd to the latest version
spm self-update

# Restart the daemon to use the new version
sudo systemctl restart spmd
```

See [usage.md](usage.md#self-update) for details.

## Build from Source

```bash
git clone https://github.com/Aloosh101/Samad-Package-Manager
cd spm
cargo build --release
sudo cp target/release/spm /usr/local/bin/spm
sudo cp target/release/spmd /usr/local/bin/spmd
```

### Requirements

- Rust 1.75+ (MSRV)
- Linux kernel 5.10+ (for sandbox features)
- systemd (optional, for daemon socket activation)

## Post-Install

```bash
# Initialize SPM
sudo spm init

# Start the daemon
sudo spmd &

# Or install as a systemd service and start it
sudo spm init --install-daemon
sudo systemctl enable --now spmd

# Verify daemon is running
spm ps

# Add a repository
spm repo add debian --source deb \
  --mirror https://deb.debian.org/debian

# Update metadata
spm update

# Install a package
spm install hello
```

## Supported Architectures

| Architecture | Asset suffix |
|---|---|
| x86_64 | `x86_64-linux-gnu` |
| aarch64 | `aarch64-linux-gnu` |
| armv7 | `armv7-linux-gnueabihf` |
| i686 | `i686-linux-gnu` |
| riscv64 | `riscv64-linux-gnu` |
