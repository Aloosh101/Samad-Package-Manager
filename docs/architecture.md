# SPM Architecture

SPM is a universal Linux package manager written in Rust. It installs `.deb`, `.rpm`, and native `.sam` packages across distributions with BLAKE3 content-addressed deduplication, PubGrub dependency resolution, Linux namespace sandboxing, Ed25519 signature verification, and atomic SQLite transactions.

---

## Design Philosophy (Theory)

| Principle | Implementation |
|-----------|---------------|
| **Pure Rust** — zero shell-out to dpkg/rpm/apt/dnf | Every format parser (ar, cpio, deb822, RPM header) written from scratch in `src/package/extract/` |
| **Atomic transactions** — install either fully succeeds or fully rolls back | `TransactionEngine` + `RollbackGuard` in `src/package/transaction.rs` |
| **Content-addressed storage** — same bytes stored once | BLAKE3 hash → `store/{origin}/{shard}/{hash}/file` in `src/store/deduplication.rs` |
| **Cross-distro sovereignty** — no vendor lock-in | `RepoSource::Deb/Rpm/Native` + smart mode with RPATH isolation in `src/package/install/mod.rs` |
| **Sandbox-first security** — isolation levels from PATH-only to full seccomp+landlock | `SandboxBuilder` in `src/package/sandbox/mod.rs` |
| **Offline-first LAN** — local cache + peer discovery | Dependency cache with TTL in `src/package/resolver/cache.rs` |
| **Transparency** — every operation logged, every file tracked | SQLite `transactions` + `files` tables in `src/db/` |

---

## Binaries

### `src/bin/spm.rs` — CLI Entry Point (25 lines)

Initialises `tracing_subscriber`, parses `SpmArgs` via clap, runs the matching command, catches `SpmError` and prints user-friendly messages via `format_user_error()`.

### `src/bin/spmd.rs` — Daemon Entry Point (24 lines)

Starts the Tokio-based daemon server loop.

---

## Module Map

```
spm CLI ──► Unix Socket ──► spmd daemon ──► TransactionEngine
                                                 │
                           ┌─────────────────────┼─────────────────────┐
                           ▼                     ▼                     ▼
                     Store (CAS)          SQLite Database      Sandbox (ns/seccomp)
```

### Source tree

```
src/
├── bin/              # spm + spmd entry points
├── cli/              # args, group, client
├── error.rs          # SpmError + SpmResult
├── types/            # package, config, version, db
├── config/           # spm.conf, repos.d, paths, libmap
├── db/               # SQLite schema + CRUD
├── package/
│   ├── install/      # install, remove, upgrade, user, local, sandbox
│   ├── fetch/        # download, verify, gs, scan
│   ├── extract/      # deb.rs, rpm.rs (pure Rust parsers)
│   ├── resolver/     # mod.rs, graph.rs, cache.rs
│   ├── sandbox/      # namespaces, seccomp, landlock, cgroups, launcher
│   ├── deb.rs
│   ├── rpm.rs
│   ├── sam.rs
│   ├── build.rs
│   ├── transaction.rs
│   ├── store.rs
│   ├── scripts.rs
│   ├── cleanup.rs
│   ├── query.rs
│   ├── hooks.rs
│   └── solver.rs
├── store/            # deduplication, hardlink, gc
├── analyze/          # orphans, conflicts, cycles
├── daemon/           # socket, permission, transaction
├── isolation/        # detector, freezer, mediator
├── ux/               # progress, prompts, formatter, completions
├── backend/          # backend resolution
├── output.rs         # terminal output helpers
├── verify.rs         # signature verification
└── util/             # hash, process, backend, fs
```

---

## 1. CLI Layer — `src/cli/`

### `args.rs` (1079 lines) — Command Definitions

Clap derive structs organising 16+ subcommands:

| Command | Purpose |
|---------|---------|
| `Install` | Install packages |
| `Remove` | Remove installed packages |
| `Purge` | Remove + delete config files |
| `Update` | Refresh repository metadata |
| `Upgrade` | Upgrade installed packages |
| `Search` | Search packages by name/pattern |
| `Info` | Show package metadata |
| `Files` | List files owned by a package |
| `Depends` | Show direct dependencies |
| `Rdepends` | Show reverse dependencies |
| `History` | Transaction log |
| `Snapshot` | Create/restore package snapshots |
| `Analyze` | Orphans, conflicts, cycles |
| `Ps` | List running daemon transactions |
| `Sandbox` | List/run sandbox environments |
| `Config` | Show/set config values |
| `Repo` | Add/remove/list repositories |
| `Gs` | Global search across repos |

Secondary enums: `AnalyzeMode` (Orphan, Conflicts), `SandboxAction` (List, Run), `ConfigAction` (Show, Set), `RepoAction` (Add, Remove, List).

### `group.rs` (394 lines) — Daemon Authorisation

Manages the `spm` UNIX group. Daemon checks `SO_PEERCRED` group membership before accepting requests.

### `client.rs` (197 lines) — Daemon Client

Connects to `/run/spm.sock`, serialises `DaemonRequest`, deserialises `DaemonResponse`. Falls back to direct execution if the daemon is unreachable.

---

## 2. Error System — `src/error.rs` (306 lines)

`SpmError` enum with 15+ variants:

| Variant | Source |
|---------|--------|
| `Io` | `std::io::Error` |
| `Database` | `rusqlite::Error` |
| `Serialization` | `serde_json` / `toml` |
| `TomlParse` / `TomlSerialize` | TOML config errors |
| `Network(String)` | HTTP/curl failures |
| `PackageNotFound` | No matching package |
| `PackageAlreadyInstalled` | Duplicate install attempt |
| `DependencyFailed` | Unsatisfiable deps |
| `InvalidFormat` | Corrupt or unknown package |
| `PermissionDenied` | Auth failure |
| `Sandbox` | Namespace/seccomp errors |
| `Config` | Config file errors |
| `CommandFailed(String)` | External command failure |
| `Other(String)` | Catch-all |

Each variant implements `Display` with user-friendly messages. `SpmResult<T>` = `Result<T, SpmError>`. `From` impls bridge rusqlite, serde_json, ureq, walkdir, toml, etc.

`format_user_error()` (in `output.rs` — see below) produces context-aware messages for common failure scenarios.

---

## 3. Data Types — `src/types/`

### `types/package.rs` — Core Domain Types

- `Package`: name, version, dependencies, conflicts, format (`Deb`, `Rpm`, `Sam`)
- `PackageId`: composite key `{name}:{format}` (e.g. `curl:deb`)
- `Dependency`, `DependencySource`, `PackageFormat`
- `Manifest`: metadata for `.sam` packages
- `PackageSource`, `AiMetadata`, `PackageSignature`

### `types/config.rs` — Configuration Types

- `RepoConfig`: source (`Apt`, `Dnf`, `Native`), priority, distro, mirrors, architectures
- `SpmConfig`: db_path, cache_path, sandbox_path, log_level, auto_snapshot
- `SandboxLevel`: `None`, `Standard`, `Strict`, `Full`
- `RepoIndex`, `RepoIndexRecord`: native repo index format

### `types/version.rs` — Version Comparison

- `Version`: epoch + version + release
- `rpmvercmp()`: RPM version comparison algorithm (used for both deb and rpm versions)
- Implements `Ord`, `PartialOrd`, `Display`, `FromStr`

### `types/db.rs` — Database Row Types

- `Transaction`, `TransactionAction`, `TransactionStatus`
- `FileRecord`, `FileAction`
- `InstalledPackage`, `InstallType`

---

## 4. Configuration — `src/config/`

### `mod.rs` (139 lines) — Config Loading

- `SpmConfig::load()`: reads `/etc/spm/spm.conf` as TOML
- `SpmConfig::set(key, value)`: atomically writes config (write to `.tmp` → rename)

### `repos.rs` (826 lines) — Repository Management

- `load_repos()`: reads `/etc/spm/repos.d/*.list` as TOML, sorted by `priority`
- `add_repo()`, `remove_repo()`: manage repo definitions
- `update_repos()`: triggers apt/dnf/native repo update
- Each `.list` file = one repository with source type, mirrors, architecture filters

### `paths.rs` (137 lines) — Path Resolution

Centralised path resolution for all SPM directories:

| Path | Purpose |
|------|---------|
| `/etc/spm/spm.conf` | Main config |
| `/etc/spm/repos.d/` | Repo definitions |
| `/var/lib/spm/metadata.db` | SQLite database |
| `/var/lib/spm/store/` | Content-addressed store |
| `/var/lib/spm/cache/` | Download cache |
| `/var/lib/spm/sandboxes/` | Sandbox roots |
| `/run/spm.sock` | Daemon socket |
| `/usr/libexec/spm/backend/` | Bundled backends |

### `libmap.rs` (179 lines) — Library Name Mapping

Cross-distro library mapping: deb ↔ rpm ↔ soname. Used by smart mode to translate library requirements between distributions.

---

## 5. Database — `src/db/`

### Schema

Singleton SQLite connection via `OnceLock<Mutex<Connection>>`. Two lock modes:
- `with_write_lock()`: exclusive file lock
- `with_read_lock()`: shared file lock

`init_schema()` creates tables + indexes + seed data.

### `packages.rs` — Installed Packages

- `get_installed_package()`, `add_installed_package()`, `remove_installed_package()`
- `list_installed_packages()`, `get_installed_packages_batch()`
- `get_store_hash()`, `get_all_installed_store_hashes()`

### `transactions.rs` — Transaction Log

- `record_transaction()`, `get_transaction()`, `list_transactions()`
- `update_transaction_status()` — marks success/failure/rollback

### `files.rs` — File Tracking

- `record_files()`: records installed file paths with store hashes
- `get_files_by_package()`, `get_files_for_transaction()`
- `get_files_by_packages_batch()`, `get_all_files()`

### `users.rs` — Per-User Installs

- `record_user_install()`, `remove_user_install()`, `list_user_installs()`
- Per-user package management (non-root installs)

### `mapping.rs` — Name/Format Mapping

- `resolve_name_mapping()`: deb → (rpm, soname)
- `resolve_rpm_to_deb()`: rpm → (deb, soname)
- `get_format_priority()`: format → priority

### `conflict.rs` (428 lines) — File Conflict Detection

- `conflict_log` table CRUD
- `detect_file_conflicts()`: HashSet intersection for file-level conflict detection
- `classify_conflicts()`: severity classification (critical/shared/minor)

---

## 6. Package Processing — `src/package/`

### `install/mod.rs` (420 lines) — Core Installer

```
install_package(name, sandbox?, replace, yes)
  → ensure_dirs
  → install_package_smart
      → detect_host_format (dpkg/rpm available?)
      → is_cross_distro(&pkg_format) → auto-enable smart mode
      → load_repos (sorted by priority)
      → candidates: repos that have the package
      → for each candidate (fallback loop):
          try_install_from_repo
            → resolve_dependencies
            → TransactionEngine::plan_install
            → TransactionEngine::display_plan
            → TransactionEngine::approve_plan
            → TransactionEngine::execute (inside write lock)
```

`autoremove_packages()`: finds orphaned packages (no dependents in DB), prompts, removes each (skips spm itself).

### `install/local.rs` (199 lines) — Local File Install

Installs `.deb`/`.rpm`/`.sam` files from disk. Parses format, extracts, records in DB.

### `install/sandbox.rs` (424 lines) — Sandbox Install

Creates an isolated symlink farm with `bwrap` integration. Files are hardlinked from the store into a sandbox root at `/var/lib/spm/sandboxes/{name}/`.

### `install/remove.rs` (275 lines) — Package Removal

Removes files, updates DB, runs postrm scripts. Supports `--purge` (remove config).

### `install/upgrade.rs` (201 lines) — Package Upgrade

Resolves new deps, plans transaction, executes via `TransactionEngine`.

### `install/user.rs` (234 lines) — User-Mode Install

Non-root installs into user's home directory (`~/.local/spm/`). Files hardlinked from global store.

### `fetch/mod.rs` (1103 lines) — Package Fetching

```
fetch_and_extract(name, version, repo):
  → fetch_apt_to_temp  (HTTP .deb from mirror)
  → fetch_dnf_to_temp  (HTTP .rpm from mirror)
  → fetch_native_to_temp (HTTP .sam from mirror)
  → verify checksums
  → return FetchedPackage
```

Cross-format fallback chain: HTTP → apt-get download → dnf download. SHA256 verification from deb822 metadata. Ed25519 signature verification for `.sam`.

### `fetch/download.rs` (231 lines) — Cache Lookup

`find_deb_in_cache()`: cached `.deb` lookup by name/version/arch.

### `fetch/gs.rs` (371 lines) — Global Search

RPM-MD repository parsing (repomd.xml, primary.xml). `download_rpm_from_repo()`: direct RPM download from mirrors. Cross-distro global package search.

### `fetch/scan.rs` (171 lines) — Directory Scan

Scans local directories for `.deb`/`.rpm`/`.sam` files.

### `fetch/verify.rs` — Cryptographic Verification

- `KeyStore`: Ed25519 verifying key storage with key_id lookup
- `Verifier`: `verify_debian()` (SHA256 from deb822), `verify_rpm()` (RPM signature header), `verify_sam()` (Ed25519 manifest signature)
- `serialize_manifest_for_verification()`: canonical manifest serialisation

### `extract/deb.rs` — Pure Rust `.deb` Extractor

Parses `ar` archive header, extracts `control.tar.*` and `data.tar.*`. Handles xz/gz/zstd compression via `xz2`/`flate2`/`zstd` crates. No `dpkg-deb` or `ar` required.

### `extract/rpm.rs` — Pure Rust `.rpm` Extractor

Parses RPM lead + header index (tag/value entries). Extracts payload via cpio newc format reader. Handles gz/xz/zstd compressed payloads. No `rpm2cpio` or `cpio` required.

### `resolver/mod.rs` — 5-Phase Resolution Pipeline

1. **Phase 1 — Fetch metadata**: fetches package metadata from repos
2. **Phase 2 — Solve dependencies**: PubGrub SAT solving
3. **Phase 3 — Detect cycles**: Tarjan SCC algorithm
4. **Phase 4 — Detect conflicts**: file-level conflict detection
5. **Phase 5 — Topological sort**: Kahn's algorithm

### `resolver/graph.rs` — Resolution Graph

- `ResolvedGraph`: topological order, cycles, alternatives, conflicts
- `Alternative`: `(PackageId, Reason)` for version alternatives
- `ConflictType`: `VersionMismatch`, `FileConflict`, `CyclicDependency`

### `resolver/cache.rs` — Dependency Cache

- `DependencyCache`: in-memory LRU with 300s TTL
- `current_dep_cache_epoch()`: bumped by `spm update` to invalidate cache

### `transaction.rs` (777 lines) — TransactionEngine

```
TransactionEngine::plan_install(deps, format)
  → TransactionPlan (packages, files, scripts)

TransactionEngine::display_plan(plan)
  → prints package table to user

TransactionEngine::approve_plan()
  → prompts user (Y/n)

TransactionEngine::execute(plan)
  → fetch_and_extract for each package (write lock held)
  → run preinst scripts
  → hardlink files from store to target
  → apply RPATH patches (smart mode)
  → record files in SQLite
  → record transaction in SQLite
  → update installed_packages table
  → run postinst scripts

RollbackGuard (Drop-based):
  → on panic: reverse files, mark transaction rolled back
```

`TransactionEngine::execute_smart()`: smart mode with RPATH isolation for cross-distro binaries.

### `store.rs` (560 lines) — Content-Addressed Store

- `store_package_dir()`: legacy path `store/{shard}/{hash}`
- `store_package_dir_for_origin()`: origin-prefixed `store/{deb,rpm,sam}/{shard}/{hash}`
- `detect_host_format()`: detects dpkg/rpm availability
- `is_cross_distro()`: true when format differs from host
- `gc_store_with_origin()` / `remove_store_with_origin()`: origin-aware GC

### `deb.rs` (308 lines) — `.deb` Parser

`parse_deb_control()`: reads deb822 control fields. `parse_deb_archive()`: iterates `ar` entries.

### `rpm.rs` (501 lines) — `.rpm` Parser

`parse_rpm_header()`: reads header index. `parse_rpm_lead()`: validates magic + arch.

### `sam.rs` (372 lines) — `.sam` Parser

Native SPM format: magic `SAM1`, zstd-compressed manifest + payload. `parse_sam_manifest()`: Ed25519-signed metadata.

### `build.rs` (347 lines) — `.sam` Builder

Builds `.sam` packages from local files. Creates manifest, signs with Ed25519, zstd-compresses payload.

### `scripts.rs` (520 lines) — Script Execution

- `run_script()`: executes script with `PR_SET_NO_NEW_PRIVS`, capability bounding set drop, rlimit, PID namespace
- `run_script_in_sandbox()`: chrooted sandbox script execution
- `save_scripts()` / `load_scripts()` / `remove_scripts()`: script storage

### `cleanup.rs` (294 lines) — System Cleanup

Removes orphan files, stale cache, temp data. `cleanup_all()`: orchestrates all cleanup tasks.

### `query.rs` (580 lines) — Package Queries

- `search()`, `info()`, `files()`, `depends()`, `rdepends()`
- `search_file_owner()`: queries SPM DB → falls back to `dpkg -S` / `rpm -qf`

### `hooks.rs` (162 lines) — Hook Scripts

Manages pre/post transaction hook scripts.

### `solver.rs` — PubGrub SAT Solver

Thin wrapper around the `pubgrub` crate. Maps package IDs to PubGrub package/version types.

---

## 7. Sandbox — `src/package/sandbox/`

### `mod.rs` — SandboxBuilder

4 isolation levels:

| Level | What it isolates |
|-------|-----------------|
| `None` | Nothing — standard install |
| `Standard` | `PATH` + `LD_LIBRARY_PATH` isolation only |
| `Strict` | Linux namespaces (PID, Mount, Network, UTS) + read-only root |
| `Full` | Everything above + seccomp BPF + Landlock LSM + cgroups v2 |

### `namespaces.rs` — Linux Namespace Isolation

- `unshare(CLONE_NEWPID)` — child becomes PID 1 (no orphan zombies)
- `unshare(CLONE_NEWNS)` + `MS_PRIVATE|MS_REC` — private mount tree
- `mount("tmpfs", "/tmp", "tmpfs")` — isolated temp directory
- `MS_REMOUNT|MS_RDONLY` — remount root read-only
- `unshare(CLONE_NEWNET)` — no network access
- `unshare(CLONE_NEWUTS)` + `sethostname("sandbox")` — hide hostname

### `seccomp.rs` — BPF Syscall Filter

- Allowlist of ~50 safe syscalls (read, write, mmap, brk, clock_gettime, etc.)
- Blocks raw I/O, module loading, kexec, user namespaces, profiling
- **Architecture support**: `AUDIT_ARCH` constants for x86_64, aarch64, arm, i686, riscv64

### `landlock.rs` — Landlock LSM Rules

- Read-only: `/usr/lib`, `/usr/share`
- Read-write: sandboxed package directory
- **Architecture support**: syscall constants cfg-gated (`i64` on 64-bit, `isize` on 32-bit)

### `cgroups.rs` — Cgroup v2 Resource Limits

- CPU quota, memory max, IO weight
- OOM killer protection
- Falls back gracefully if cgroups unavailable

### `launcher.rs` — Process Launcher

Forks child in new namespaces, applies seccomp + landlock + cgroups before `execvp`. Falls back gracefully if any feature is unavailable on the running kernel.

---

## 8. Content-Addressed Store — `src/store/`

### `deduplication.rs` — Core Dedup

- Files stored by BLAKE3 hash, sharded by first 2 hex chars
- Path: `store/{origin}/{shard}/{hash}/file`
- Same content → same hash → zero extra disk space
- Origin prefixes (`deb`, `rpm`, `sam`) keep store organised

### `hardlink.rs` — Hardlink Management

- Hardlinks store paths to target directories (`/usr/bin/`, `/usr/lib/`, etc.)
- Maintains link reference counts in SQLite
- Safe cleanup on GC (only removes store entries when refcount reaches zero)

### `gc.rs` — Garbage Collection

- Scans store for unreferenced hashes
- Respects origin prefixes
- Dry-run mode (`--dry-run`) for preview

---

## 9. Analysis — `src/analyze/mod.rs` (419 lines)

- `full_analysis()`: runs orphans + conflicts + dependency cycles
- `find_orphans()`: dependency graph via SQLite → packages with no dependents
- `find_conflicts()`: file-level conflict detection between all installed packages
- `find_dependency_cycles()`: DFS with visited + recursion stack
- `trace_binary()`: runs `ldd` → suggests missing libraries via apt/dnf

---

## 10. Daemon — `src/daemon/`

### `mod.rs` (754 lines) — Daemon Server

- Tokio-based UNIX socket server at `/run/spm.sock`
- `SO_PEERCRED` kernel-level authentication (no token, no password)
- Rate limiter: max 10 req/s per user
- Concurrency limiter: max 3 concurrent transactions per user
- Signal handling via `tokio::signal` + `select!` loop
- Request types: install, remove, upgrade, repo management

### `permission.rs` — Permission Engine

- `PeerCreds`: UID/PID/GID from `SO_PEERCRED`
- `PermissionPolicy`: allowed operations per user
- `check_permission()`: validates file access and operations
- Membership check against `spm` UNIX group

### `transaction.rs` — Transaction Manager

6-phase atomic install:
1. Fetch + verify package
2. Detect conflicts
3. Prepare store (deduplicate, hardlink)
4. Pre-install script execution
5. Database commit (atomic SQLite)
6. Post-install script execution

`RollbackGuard`: Drop-based rollback on panic/failure.

---

## 11. Isolation — `src/isolation/`

### `detector.rs` — PackageManagerDetector

- Probes for dpkg/rpm/pacman on the system
- Watches for foreign package manager invocations via `inotify`

### `freezer.rs` — ProcessFreezer

- `SIGSTOP` / `SIGCONT` for process suspension
- `SIGKILL` for forceful termination
- Tracks frozen process list

### `mediator.rs` — ConflictMediator

- Detects file conflicts between SPM and foreign packages
- Presents resolution options: Allow, Deny, Sandbox
- Integrates with `ux::prompts::SpmPrompts`

---

## 12. User Experience — `src/ux/`

### `progress.rs` — SpmProgress

- `MultiProgress` via `indicatif`
- `progress_bar()`: per-operation progress bars
- `spinner()`: indeterminate operation spinner
- `set_message()` / `finish_with_message()`

### `prompts.rs` — SpmPrompts

- `confirm_install()`: package list confirmation
- `select_conflict_resolution()`: conflict resolution picker
- `select_foreign_touch_action()`: foreign process action picker
- Uses `dialoguer` with coloured themes

### `formatter.rs` — SpmFormatter

- `print_install_summary()`: formatted install results
- `print_error_actionable()`: error with fix suggestions
- Coloured output via `colored` crate

### `completions.rs` — Shell Completions

Generates bash/zsh/fish completion scripts via `clap_complete`.

---

## 13. Backend Management — `src/backend/mod.rs` (160 lines)

### Backend Resolution Order

```
resolve("dnf"):
  1. /var/lib/spm/store/backend/dnf/bin/dnf    ← managed copy
  2. /usr/libexec/spm/backend/dnf               ← bundled with SPM
  3. /dev/null/spm-backend-missing              ← fail cleanly (never system PATH)
```

- `check_missing()`: detects which backends are required for the host distribution and whether they're available in store or bundled
- `show_warnings()`: called at the start of every `spm` command if any backends are missing; displays once per process via `OnceLock`
- `copy_bundled_to_store()`: copies bundled backends to store
- `download_backend()`: downloads backend binary from URL
- `list_store_backends()`: enumerates backends in store

---

## 14. Utilities — `src/util/`

### `hash.rs` (117 lines)

- `hash_file()` / `hash_bytes()`: BLAKE3 hashing
- `sha256_hex()`: SHA256 hex digest

### `process.rs` (183 lines)

- `find_deleted_libs()`: scans `/proc/*/maps` for `(deleted)` shared libraries
- Maps deleted libs to packages via dpkg/rpm

### `backend.rs` (80 lines)

Backend binary resolution (never system PATH):

```rust
resolve("dnf")
  → store path
  → bundled path
  → /dev/null/spm-backend-missing
```

### `fs.rs` (61 lines)

Atomic write + lock file helpers.

---

## 15. Verification — `src/verify.rs` (132 lines)

- `verify_manifest_signature()`: Ed25519 signature verification for `.sam` manifests
- `prompt_before_install()`: user confirmation prompt

---

## 16. Terminal Output — `src/output.rs` (446 lines)

- `section()`, `step()`, `step_warn()`, `step_success()`: progress display helpers
- `cyan()`, `dim()`: terminal styling
- `format_user_error()`: user-friendly error formatting

---

## Data Flow: `spm install curl`

```
1. cli/args.rs → install::install_package("curl", None, false, false)

2. install_package_smart:
   a. load_repos() — sorted by priority
   b. Find candidates: repos containing "curl"
   c. For first candidate:
      - resolve_dependencies("curl:deb") → ResolvedGraph
      - TransactionEngine::plan_install → TransactionPlan
      - display_plan → approve_plan
      - TransactionEngine::execute (write lock):
        - fetch_and_extract (HTTP from mirror)
        - verify (SHA256 or Ed25519)
        - extract (ar → data.tar.zst → /)
        - run preinst scripts
        - record files in SQLite
        - record transaction in SQLite
        - add to installed_packages
        - run postinst scripts
   d. If fetch fails → try next repo candidate

3. SpmResult::Ok(())
```

On error:
- Package not found → `SpmError::PackageNotFound`
- Already installed → `SpmError::PackageAlreadyInstalled`
- Network failure → `SpmError::Network`
- → `format_user_error()` → user-friendly message

---

## Directory Layout

```
/var/lib/spm/
├── store/
│   ├── deb/{shard}/{hash}/file     # Debian package contents
│   ├── rpm/{shard}/{hash}/file     # RPM package contents
│   └── sam/{shard}/{hash}/file     # Native SPM package contents
├── metadata.db                      # SQLite database
├── cache/                           # Download cache
├── sandboxes/                       # Sandbox environments
└── repos.d/                         # Repository configurations

/etc/spm/
├── spm.conf                         # Main configuration (TOML)
└── repos.d/
    └── {name}.list                  # Repository definitions (TOML)

/run/spm.sock                        # Daemon Unix socket

/usr/libexec/spm/backend/
├── dpkg                             # Bundled dpkg backend
├── rpm                              # Bundled rpm backend
└── dnf                              # Bundled dnf backend
```
