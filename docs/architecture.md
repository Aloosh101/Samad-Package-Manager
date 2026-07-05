# Architecture

SPM consists of two binaries: `spm` (CLI) and `spmd` (daemon).

## Overview

```
spm (CLI) ──► Unix Socket ──► spmd (daemon) ──► Install/Remove/Query
                                                  │
                                            Store (BLAKE3 CAS)
                                                  │
                                            SQLite Database
```

## Key Components

### CLI (`spm`)
- Command-line interface for all package operations
- Communicates with `spmd` via Unix socket (`/run/spm.sock`)
- Falls back to direct execution if daemon is unavailable

### Daemon (`spmd`)
- Tokio async daemon with `SO_PEERCRED` authentication
- Manages concurrent transactions with rate limiting
- Orchestrates install, remove, and upgrade operations

### Content-Addressed Store
- BLAKE3 hashing for deduplication
- Hardlink-based storage to save disk space
- Garbage collection for orphaned store paths

### Package Formats
- **`.deb`** — Pure Rust AR archive parser + XZ/Zstd/gzip decompression
- **`.rpm`** — Pure Rust CPIO archive extraction (no rpm2cpio dependency)
- **`.sam`** — Native SPM package format

### Dependency Resolution
- PubGrub SAT solver with TTL cache
- Tarjan SCC for cycle detection
- SONAME indexing for shared library tracking

### Sandboxing
- Linux namespace isolation (PID/Mount/Network/UTS)
- Seccomp BPF syscall filtering
- Landlock LSM integration
- Cgroup v2 resource limits

## Directory Layout

```
/var/lib/spm/
├── store/          # Content-addressed package store
│   ├── deb/        # Debian packages
│   ├── rpm/        # RPM packages
│   └── sam/        # Native SPM packages
├── metadata.db     # SQLite database
├── cache/          # Download cache
├── sandboxes/      # Sandbox environments
└── repos.d/        # Repository configurations
```
