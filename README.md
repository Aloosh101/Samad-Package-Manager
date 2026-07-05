# SPM — Samad Package Manager

<p align="center">
  <img src="https://img.shields.io/badge/version-0.3.0-blue" alt="Version">
  <img src="https://img.shields.io/badge/rustc-2021+-orange" alt="Rust Edition">
  <img src="https://img.shields.io/badge/license-PolyForm%20Noncommercial-green" alt="License">
  <img src="https://img.shields.io/badge/status-alpha-yellow" alt="Status">
</p>

> A universal Linux package manager with content-addressed storage, cross-distro support, and sandbox isolation.

> *"Good artists copy; great artists steal."* — Picasso  
> *"Don't reinvent the wheel. Make it better, faster, and more efficient."*

SPM does not try to compete with apt, dnf, or Nix. It steals their best ideas and delivers a whole greater than the sum of its parts.

SPM manages packages from `.deb`, `.rpm`, and its native `.sam` formats through a single CLI, backed by a BLAKE3 content-addressed store, PubGrub dependency resolution, Linux namespace sandboxing, and a Unix-socket daemon with `SO_PEERCRED` kernel-level authentication.

---

## Quick Install

```bash
curl -fsSL https://github.com/Aloosh101/Samad-Package-Manager/releases/latest/download/install.sh | sudo bash
```

This auto-detects your architecture and installs the pre-built binary. No Rust toolchain required.

---

## Documentation

| Topic | File |
|-------|------|
| Installation & setup | [docs/installation.md](docs/installation.md) |
| Architecture overview | [docs/architecture.md](docs/architecture.md) |
| Principles & philosophy | [docs/principles.md](docs/principles.md) |
| Usage & commands | [docs/usage.md](docs/usage.md) |
| Contributing | [docs/contributing.md](docs/contributing.md) |

---

## Features

| Capability | Status |
|---|---|
| `.deb` / `.rpm` / `.sam` extraction (pure Rust, no external tools) | ✅ |
| BLAKE3 content-addressed store with deduplication | ✅ |
| PubGrub SAT dependency solver with TTL cache | ✅ |
| Linux namespace sandboxing (PID / Mount / Network / UTS) | ✅ |
| Seccomp BPF syscall filtering | ✅ |
| Landlock / Cgroup v2 integration | ✅ |
| Ed25519 signature + SHA256 verification | ✅ |
| SO_PEERCRED daemon authentication | ✅ |
| Transaction-based atomic install with rollback | ✅ |
| File conflict detection + resolution | ✅ |
| Foreign package manager detection (dpkg/rpm/pacman) | ✅ |
| Cross-distro install (`.deb` on Fedora, `.rpm` on Debian) | ✅ |
| Offline LAN distribution via daemon socket | ✅ |
| Btrfs snapshot integration | ✅ |
| Shell completions (bash/zsh/fish) | ✅ |
| Systemd socket activation | ✅ |

---

## Quick Start

```bash
# Install
curl -fsSL https://github.com/Aloosh101/Samad-Package-Manager/releases/latest/download/install.sh | sudo bash

# Add Debian repo
spm repo add debian --source deb \
  --mirrors https://deb.debian.org/debian \
  --codename stable --components main

# Update metadata
spm update

# Install a package
spm install figlet
```

---

## Build from Source

```bash
git clone https://github.com/Aloosh101/Samad-Package-Manager && cd spm
cargo build --release
sudo cp target/release/spm /usr/local/bin/spm
sudo cp target/release/spmd /usr/local/bin/spmd
```

### Requirements

- Rust 1.75+ (MSRV)
- Linux kernel 5.10+ (for full sandbox features)
- SQLite 3.x (bundled, no system dep)
- Optional: systemd (for socket activation)

---

## Architecture

```
                       spm (CLI)
                          │
               ┌──────────┴──────────┐
               │                     │
               ▼                     ▼
           Unix Socket          Direct Execution
         /run/spm.sock         (fallback path)
               │
          ┌────┴────┐
          │  spmd   │  Tokio async daemon
          └────┬────┘
               │
          ┌────┴────┐
          │ Auth    │  SO_PEERCRED · rate-limit · concurrency
          └────┬────┘
               │
     ┌─────────┴─────────────┐
     │                       │
     ▼                       ▼
 System Install          User Install
(root / spm group)      (~/.local)
     │                       │
     ▼                       ▼
/var/lib/spm/          ~/.local/share/spm/
  ├─ store/{deb,rpm,sam}/  ├─ store/
  ├─ metadata.db            ├─ history.sqlite
  └─ cache/                 └─ bin/
```

See [docs/architecture.md](docs/architecture.md) for a full breakdown.

---

## Build & Test

```bash
cargo check          # No codegen
cargo build          # Debug build
cargo build --release
cargo test --lib     # Run all lib tests
cargo clippy --no-deps
```

---

## Project Status

SPM is in **active development** (v0.3.0). All modules compile cleanly and core
package operations (install, remove, update, search, info, files, history) work
end-to-end with Debian repositories.

---

## License

**PolyForm Noncommercial License 1.0.0** — free for non-commercial use. Commercial use requires permission. See [LICENSE](LICENSE) for details.

---

<p align="center">
  Built with <a href="https://opencode.ai">opencode</a>
</p>
