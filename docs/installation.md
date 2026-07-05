# Installation

## Quick Install (via script)

```bash
curl -fsSL https://github.com/Aloosh101/Samad-Package-Manager/releases/latest/download/install.sh | bash
```

This auto-detects your architecture and OS, downloads the correct binary, and installs
SPM to `/usr/local/bin`.

### Options

```bash
# System-wide install (default)
curl -fsSL https://github.com/Aloosh101/Samad-Package-Manager/releases/latest/download/install.sh | bash

# User install (~/.local/bin)
curl -fsSL https://github.com/Aloosh101/Samad-Package-Manager/releases/latest/download/install.sh | bash -s -- --user
```

## Manual Install

Download the binary for your architecture from the
[releases page](https://github.com/Aloosh101/Samad-Package-Manager/releases):

```bash
# Example: x86_64 Linux
wget https://github.com/Aloosh101/Samad-Package-Manager/releases/latest/download/spm-x86_64-linux-gnu
chmod +x spm-x86_64-linux-gnu
sudo mv spm-x86_64-linux-gnu /usr/local/bin/spm
```

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
- systemd (optional, for socket activation)

## Post-Install

```bash
# Initialize SPM
spm init

# Add a repository
spm repo add debian --source deb --mirrors https://deb.debian.org/debian --codename stable --components main

# Update metadata
spm update

# Install a package
spm install hello
```
