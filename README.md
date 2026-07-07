# SPM — Samad Package Manager

<p align="center">
  <img src="https://img.shields.io/badge/version-0.3.5-blue" alt="Version">
  <img src="https://img.shields.io/badge/rustc-2021+-orange" alt="Rust Edition">
  <img src="https://img.shields.io/badge/license-PolyForm%20Noncommercial-green" alt="License">
  <img src="https://img.shields.io/badge/status-beta-green" alt="Status">
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
| No dependency on system package managers (dpkg/rpm/apt/dnf) | ✅ |
| Cross-distro install (`.deb` on Fedora, `.rpm` on Debian) | ✅ |
| Thin-client daemon architecture (`spm` → socket → `spmd`) | ✅ |
| Self-update (`spm self-update`) | ✅ |
| Offline LAN distribution via daemon socket | ✅ |
| Btrfs snapshot integration | ✅ |
| Shell completions (bash/zsh/fish) | ✅ |
| Systemd socket activation | ✅ |
| Sandbox list / run (`spm sandbox`) | ✅ |
| Hardware detection (`spm detect`) | ✅ |
| Package build (`spm build`) | ✅ |
| Package conversion (`spm install --convert-only`) | ✅ |
| `.sam` native format packaging | ✅ |
| Repository signing (Ed25519) | ✅ |
| Integrity check (`spm fsck`) | ✅ |
| System sync (`spm sync`) | ✅ |
| Process deleted-libs check (`spm ps`) | ✅ |

---

## Quick Start

```bash
# Install
curl -fsSL https://github.com/Aloosh101/Samad-Package-Manager/releases/latest/download/install.sh | sudo bash

# Add Debian repo
spm repo add debian --source deb \
  --mirror https://deb.debian.org/debian

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
sudo spmd                           # Start the daemon (required for all operations)
```

### Requirements

- Rust 1.75+ (MSRV)
- Linux kernel 5.10+ (for full sandbox features)
- SQLite 3.x (bundled, no system dep)
- systemd (optional, for socket activation / daemon service)

---

## Architecture (Thin Client Model)

```
                     spm (CLI)
                    thin client
                        │
                        ▼
                  Unix Socket            ← if spmd is down, spm fails
                /run/spm.sock
                        │
                   ┌────┴────┐
                   │  spmd   │  Tokio async daemon (root)
                   └────┬────┘
                        │
                  ┌─────┴─────┐
                  │ Auth      │  SO_PEERCRED · rate-limit · concurrency
                  └─────┬─────┘
                        │
               ┌────────┴────────┐
               │                 │
               ▼                 ▼
         System Install      User Install
        (root / spm group)   (~/.local)
               │                 │
               ▼                 ▼
         /var/lib/spm/      ~/.local/share/spm/
           ├─ store/          ├─ store/
           ├─ metadata.db     ├─ history.sqlite
           └─ cache/          └─ bin/
```

See [docs/architecture.md](docs/architecture.md) for a full breakdown.

---

## Build & Test

```bash
cargo check                # No codegen
cargo build                # Debug build
cargo build --release
cargo test --lib           # Unit tests (482+)
cargo test --lib -- --nocapture  # With stdout
cargo clippy --no-deps     # No warnings
```

---

## Project Status

SPM is in **beta** (v0.3.5). The core feature set is complete: all 27+ package
operations (install, remove, update, upgrade, search, info, files, history,
sandbox, repo management, analysis, self-update, and more) work through the
spmd daemon across 5 architectures (x86_64, aarch64, armv7, i686, riscv64).

### v0.3.5 — Pre-install Conflict Detection
- **Physical path scan before download**: checks `/usr/bin`, `/bin`, `/usr/local/bin`, `/usr/sbin`, `/sbin`, `/opt`, `/usr/games`, `~/.local/bin`, `~/bin` for existing files matching the package name
- **Catastrophic --replace warning**: big red warning banner when forcing overwrite of files possibly owned by other package managers
- **--replace passthrough fix**: the `--replace` flag now correctly reaches the daemon for normal `spm install` commands

---

## License

**PolyForm Noncommercial License 1.0.0** — free for non-commercial use. Commercial use requires permission. See [LICENSE](LICENSE) for details.

---

<p align="center">
  Built with <a href="https://opencode.ai">opencode</a>
</p>
