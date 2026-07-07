# SPM Architecture

SPM is a universal Linux package manager written in Rust. It installs `.deb`, `.rpm`, and native `.sam` packages across distributions with BLAKE3 content-addressed deduplication, PubGrub dependency resolution, Linux namespace sandboxing, Ed25519 signature verification, and atomic SQLite transactions.

**Thin-client model**: `spm` (CLI) never executes operations directly — every command is sent via a Unix socket to `spmd` (daemon), which authenticates via `SO_PEERCRED`, rate-limits, and executes. If `spmd` is unreachable, `spm` fails immediately. There is no fallback to direct execution.

---

## Design Philosophy (Theory)

| Principle | Implementation |
|-----------|---------------|
| **Pure Rust** — zero shell-out to dpkg/rpm/apt/dnf | Every format parser (ar, cpio, deb822, RPM header) written from scratch in `src/package/extract/` |
| **Atomic transactions** — install either fully succeeds or fully rolls back | `TransactionEngine` + `RollbackGuard` in `src/package/transaction.rs` |
| **Content-addressed storage** — same bytes stored once | BLAKE3 hash → `store/{origin}/{shard}/{hash}/file` in `src/store/deduplication.rs` |
| **Cross-distro sovereignty** — no vendor lock-in | `RepoSource::Deb/Rpm/Native` + smart mode with RPATH isolation in `src/package/install/mod.rs` |
| **Sandbox-first security** — isolation levels from PATH-only to full seccomp+landlock | `SandboxBuilder` in `src/package/sandbox/mod.rs` |
| **Thin client** — `spm` is a pure relay to `spmd` | All 27+ commands sent as JSON over `/run/spm.sock`; `spm` fails if daemon is down |
| **Offline-first LAN** — local cache + peer discovery | Dependency cache with TTL in `src/package/resolver/cache.rs` |
| **Transparency** — every operation logged, every file tracked | SQLite `transactions` + `files` tables in `src/db/` |

---

## Binaries

### `src/bin/spm.rs` — CLI Entry Point (25 lines)

Initialises `tracing_subscriber`, parses `SpmArgs` via clap, sends the command as JSON to `spmd` via `/run/spm.sock`, and prints the response. If `spmd` is unreachable, prints an error and exits.

### `src/bin/spmd.rs` — Daemon Entry Point (24 lines)

Starts the Tokio-based daemon server loop. Must run as root. Listens on `/run/spm.sock`, authenticates via `SO_PEERCRED`, dispatches to handlers.

---

## Module Map

```
spm (CLI) ──► Unix Socket ──► spmd (daemon)
                                   │
                      ┌────────────┴────────────┐
                      │                         │
                 TransactionEngine          Auth/RateLimit
                      │                         │
         ┌────────────┼────────────┐            │
         ▼            ▼            ▼            │
   Store (CAS)   SQLite DB    Sandbox           │
                              (ns/seccomp)      │
         └────────────┬────────────┘            │
                      ▼                         ▼
               /var/lib/spm/            SO_PEERCRED
```

### Source tree

```
src/
├── bin/              # spm + spmd entry points
├── cli/              # args, client, group
│   ├── args.rs       # clap command definitions + dispatch
│   ├── client.rs     # send_command() → JSON over Unix socket
│   └── group.rs      # spm UNIX group management
├── daemon/
│   ├── mod.rs        # server loop, auth, dispatch, 27+ handlers
│   ├── conflict.rs   # file conflict detection
│   ├── ipc.rs        # IPC protocol types
│   ├── monitor.rs    # transaction monitoring
│   ├── permission.rs # permission engine
│   ├── store.rs      # daemon-side store helpers
│   └── transaction.rs# transaction coordinator
├── error.rs          # SpmError + SpmResult
├── types/
│   ├── package.rs    # Package, Dependency, PackageFormat
│   ├── config.rs     # SpmConfig, RepoConfig
│   ├── version.rs    # rpmvercmp() version comparison
│   └── db.rs         # InstalledPackage, Transaction, FileRecord
├── config/
│   ├── mod.rs        # SpmConfig::load/set
│   ├── repos.rs      # RepoConfig CRUD, update, create, sign
│   ├── paths.rs      # all SPM directory paths
│   └── libmap.rs     # cross-distro library name mapping
├── db/
│   ├── mod.rs        # SQLite connection + schema init
│   ├── packages.rs   # installed_packages CRUD
│   ├── transactions.rs# transaction log
│   ├── files.rs      # file tracking
│   ├── users.rs      # per-user installs
│   ├── mapping.rs    # name/format resolution
│   └── conflict.rs   # conflict log
├── package/
│   ├── install/
│   │   ├── mod.rs    # install_package, install_package_smart
│   │   ├── remove.rs # remove_package, purge_package
│   │   ├── upgrade.rs# upgrade_package, dist_upgrade_packages
│   │   ├── local.rs  # install_local_package
│   │   ├── sandbox.rs# sandbox install
│   │   └── user.rs   # per-user installs
│   ├── fetch/
│   │   ├── mod.rs    # fetch_and_extract
│   │   ├── download.rs# HTTP download with resume
│   │   ├── verify.rs # SHA256 / Ed25519 verification
│   │   ├── gs.rs     # global search (Debian/COPR)
│   │   └── scan.rs   # local directory scan
│   ├── extract/
│   │   ├── deb.rs    # pure Rust .deb parser
│   │   └── rpm.rs    # pure Rust .rpm parser
│   ├── resolver/
│   │   ├── mod.rs    # 5-phase resolution pipeline
│   │   ├── graph.rs  # ResolvedGraph + ConflictType
│   │   └── cache.rs  # LRU dependency cache (300s TTL)
│   ├── sandbox/
│   │   ├── mod.rs    # SandboxBuilder (4 isolation levels)
│   │   ├── namespaces.rs# CLONE_NEW* unshare + mount
│   │   ├── seccomp.rs # BPF syscall whitelist (~50 syscalls)
│   │   ├── landlock.rs# LSM file access rules
│   │   ├── cgroups.rs# cgroup v2 resource limits
│   │   └── launcher.rs# fork + exec in namespaces
│   ├── deb.rs        # .deb control parser
│   ├── rpm.rs        # .rpm header parser
│   ├── sam.rs        # .sam format (convert_to_sam)
│   ├── build.rs      # .sam package builder
│   ├── transaction.rs# TransactionEngine (plan → execute → rollback)
│   ├── store.rs      # store paths + host detection
│   ├── scripts.rs    # pre/post install script execution
│   ├── cleanup.rs    # orphan files, stale cache
│   ├── query.rs      # search, info, files, depends, rdepends, history
│   ├── hooks.rs      # hook script management
│   └── solver.rs     # PubGrub SAT solver wrapper
├── store/
│   ├── deduplication.rs# BLAKE3 content-addressed store
│   ├── hardlink.rs   # hardlink management + refcounts
│   └── gc.rs         # garbage collection
├── analyze/
│   ├── mod.rs        # orphans, conflicts, cycles, trace_binary
├── isolation/
│   ├── detector.rs   # foreign package manager detection
│   ├── freezer.rs    # SIGSTOP/SIGCONT process suspension
│   └── mediator.rs   # conflict resolution UI
├── sandbox/
│   └── desktop.rs    # sandbox desktop entry creation
├── ux/
│   ├── progress.rs   # indicatif MultiProgress bars
│   ├── prompts.rs    # dialoguer prompts
│   ├── formatter.rs  # coloured output
│   └── completions.rs# clap_complete shell completions
├── output.rs         # terminal output helpers (section, step, colors)
├── verify.rs         # Ed25519 signature verification
├── fsck.rs           # integrity check
├── sync.rs           # DB ↔ filesystem sync
├── kernel.rs         # GPU detection, package suggestions
├── bootstrap.rs      # init_system, install_daemon_service
├── index.rs          # SONAME index build
├── backend/
│   └── mod.rs        # backend binary resolution
└── util/
    ├── hash.rs       # BLAKE3 + SHA256 helpers
    ├── process.rs    # /proc scanning, deleted libs
    ├── backend.rs    # backend path resolution
    ├── fs.rs         # atomic write + lock helpers
    └── user.rs       # user name resolution
```

---

## 1. CLI Layer — `src/cli/`

### `args.rs` (~800 lines) — Command Definitions + Dispatch

Clap derive structs organising 30+ subcommands. Every command is dispatched via `client::send_command(action, params)` which serialises the request and sends it to `spmd` over a Unix socket. The CLI never executes package operations directly.

| Command | Alias | Purpose |
|---------|-------|---------|
| `Install` | `i` | Install packages (repo, local, sandbox) |
| `Remove` | `r` | Remove installed packages |
| `Purge` | `p` | Remove + config files |
| `Autoremove` | — | Remove orphaned dependencies |
| `Update` | `u` | Refresh repository metadata |
| `Upgrade` | `upg` | Upgrade packages |
| `DistUpgrade` | — | Full distribution upgrade |
| `Search` | `s` | Local package search |
| `GlobalSearch` | `gs` | Remote search (Debian/COPR) |
| `SearchFile` | — | Find package owning a file |
| `Info` | `show` | Package metadata |
| `Files` | `ls` | List package files |
| `Depends` | `deps` | Show dependencies |
| `Rdepends` | `rdeps` | Show reverse dependencies |
| `History` | `log` | Transaction log / undo |
| `Snapshot` | `snap` | Btrfs snapshots |
| `Analyze` | `an` | Orphans, conflicts, binary trace |
| `Ps` | — | Processes using deleted libs |
| `Sandbox` | — | List / run sandboxes |
| `SelfUpdate` | — | Update spm + spmd from GitHub |
| `Repo` | — | Manage repositories |
| `RepoList` | — | List repositories |
| `RepoCreate` | — | Create a repository |
| `RepoPublish` | — | Publish to a repository |
| `RepoGenKey` | — | Generate signing key |
| `RepoSign` | — | Sign a repository |
| `Index` | — | Rebuild SONAME index |
| `Detect` | — | Hardware detection |
| `Build` | `b` | Build .sam packages |
| `Cleanup` | — | Clean orphaned files / cache |
| `Config` | — | Show / set config |
| `Group` | — | Manage spm UNIX group |
| `Sync` | — | Sync DB with system state |
| `Fsck` | — | Integrity check |
| `Init` | — | Bootstrap fresh system |

### `client.rs` (~130 lines) — Daemon Client

- `send_command(action, params)` — serialises `ClientRequest` (action, package, params, user_id, user_home) as JSON, sends to `/run/spm.sock`, reads streaming progress lines and final response
- `send_request_action(action, package)` — simpler interface for read-only commands (search, info, files, etc.)
- `send_request_raw(req)` — low-level socket I/O; reconnects on each call; handles progress messages on stderr
- **No fallback** — if socket connect fails, returns `SpmError` immediately

### `group.rs` (~394 lines) — Daemon Authorisation

Manages the `spm` UNIX group. Daemon checks `SO_PEERCRED` group membership before accepting requests.

---

## 2. Error System — `src/error.rs` (~310 lines)

`SpmError` enum with 15+ variants, each implementing `Display` with user-friendly messages. `SpmResult<T>` = `Result<T, SpmError>`. `From` impls bridge rusqlite, serde_json, ureq, walkdir, toml, etc.

| Variant | Source |
|---------|--------|
| `Io` | `std::io::Error` |
| `Database` | `rusqlite::Error` |
| `Serialization` | `serde_json` / `toml` |
| `TomlParse` / `TomlSerialize` | TOML config errors |
| `Network(String)` | HTTP/ureq failures |
| `PackageNotFound` | No matching package |
| `PackageAlreadyInstalled` | Duplicate install attempt |
| `DependencyFailed` | Unsatisfiable deps |
| `InvalidFormat` | Corrupt or unknown package |
| `PermissionDenied` | Auth failure |
| `Sandbox` | Namespace/seccomp errors |
| `Config` | Config file errors |
| `CommandFailed(String)` | External command failure |
| `Other(String)` | Catch-all |

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

### `mod.rs` (~140 lines) — Config Loading
- `SpmConfig::load()`: reads `/etc/spm/spm.conf` as TOML
- `SpmConfig::set(key, value)`: atomically writes config (write to `.tmp` → rename)

### `repos.rs` (~1070 lines) — Repository Management
- `load_repos()`: reads `/etc/spm/repos.d/*.list` as TOML, sorted by `priority`
- `add_repo()`, `remove_repo()`: manage repo definitions
- `update_repos()`: triggers apt/dnf/native repo update
- `create_repo()`: initialise a new repo directory structure
- `publish_package()`: copy + sign a package into a repo
- `generate_signing_key()`: Ed25519 key pair generation
- `sign_repo()`: sign repo index with Ed25519
- Each `.list` file = one repository with source type, mirrors, architecture filters

### `paths.rs` (~140 lines) — Path Resolution
Centralised path resolution for all SPM directories.

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

### `libmap.rs` (~180 lines) — Library Name Mapping
Cross-distro library mapping: deb ↔ rpm ↔ soname. Used by smart mode to translate library requirements between distributions.

---

## 5. Database — `src/db/`

Singleton SQLite connection via `OnceLock<Mutex<Connection>>`. Two lock modes:
- `with_write_lock()`: exclusive file lock
- `with_read_lock()`: shared file lock

`init_schema()` creates tables + indexes + seed data.

| Module | Purpose |
|--------|---------|
| `packages.rs` | `get_installed_package()`, `add_installed_package()`, `remove_installed_package()`, `list_installed_packages()` |
| `transactions.rs` | `record_transaction()`, `get_transaction()`, `list_transactions()`, `update_transaction_status()` |
| `files.rs` | `record_files()`, `get_files_by_package()`, `get_files_for_transaction()`, `get_all_files()` |
| `users.rs` | `record_user_install()`, `remove_user_install()`, `list_user_installs()` |
| `mapping.rs` | `resolve_name_mapping()`, `resolve_rpm_to_deb()`, `get_format_priority()` |
| `conflict.rs` | `conflict_log` table CRUD, `detect_file_conflicts()` |

---

## 6. Package Processing — `src/package/`

### `install/mod.rs` (~420 lines) — Core Installer
```
install_package(name, sandbox?, replace, yes, smart, strategy)
  → ensure_dirs
  → install_package_smart
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
`autoremove_packages()`: finds orphaned packages, removes each (skips spm itself).

### `install/remove.rs` (~275 lines) — Package Removal
Removes files, updates DB, runs postrm scripts. Supports `--purge` (remove config).

### `install/upgrade.rs` (~320 lines) — Package Upgrade
`upgrade_package()`: resolves new deps, plans transaction, executes via `TransactionEngine`. `dist_upgrade_packages()`: full distribution upgrade.

### `install/local.rs` (~200 lines) — Local File Install
Installs `.deb`/`.rpm`/`.sam` files from disk. Parses format, extracts, records in DB.

### `install/sandbox.rs` (~425 lines) — Sandbox Install
Creates an isolated symlink farm. Files are hardlinked from the store into `/var/lib/spm/sandboxes/{name}/`.

### `install/user.rs` (~235 lines) — User-Mode Install
Non-root installs into user's home directory (`~/.local/spm/`). Files hardlinked from global store.

### `fetch/mod.rs` (~1100 lines) — Package Fetching
```
fetch_and_extract(name, version, repo):
  → fetch_apt_to_temp  (HTTP .deb from mirror)
  → fetch_dnf_to_temp  (HTTP .rpm from mirror)
  → fetch_native_to_temp (HTTP .sam from mirror)
  → verify checksums
  → return FetchedPackage
```
Cross-format fallback chain: HTTP → apt-get download → dnf download.

### `extract/deb.rs` — Pure Rust `.deb` Extractor
Parses `ar` archive header, extracts `control.tar.*` and `data.tar.*`. Handles xz/gz/zstd compression. No `dpkg-deb` required.

### `extract/rpm.rs` — Pure Rust `.rpm` Extractor
Parses RPM lead + header index. Extracts payload via cpio newc format reader. No `rpm2cpio` required.

### `resolver/mod.rs` — 5-Phase Resolution Pipeline
1. Fetch metadata → 2. PubGrub solve → 3. Tarjan cycle detection → 4. File conflict detection → 5. Kahn topological sort

### `transaction.rs` (~780 lines) — TransactionEngine
```
TransactionEngine::plan_install(deps, format) → TransactionPlan
TransactionEngine::display_plan(plan)         → prints table
TransactionEngine::approve_plan()             → prompts user
TransactionEngine::execute(plan)              → fetch, verify, extract, hardlink, record, run scripts
```
`RollbackGuard`: Drop-based rollback on panic/failure.

### `scripts.rs` (~520 lines) — Script Execution
- `run_script()`: executes with `PR_SET_NO_NEW_PRIVS`, capability drop, rlimit, PID namespace
- `run_script_in_sandbox()`: chrooted sandbox + `unshare(CLONE_NEWNS|NEWPID)` + capability drop + `setrlimit` in `pre_exec`

### `query.rs` (~580 lines) — Package Queries
- `search_packages()`, `package_info()`, `list_package_files()`
- `package_dependencies()`, `reverse_dependencies()`
- `search_file_owner()`: queries SPM DB → falls back to dpkg/rpm
- `create_snapshot()`, `rollback_snapshot()`, `show_history()`, `undo_transaction()`

### `cleanup.rs` (~295 lines) — System Cleanup
`cleanup_all(force)`: removes orphan files, stale cache, temp data.

### `sam.rs` (~375 lines) — `.sam` Format
Native SPM format: magic `SAM1`, zstd-compressed manifest + payload. `convert_to_sam()`: converts `.deb`/`.rpm` → `.sam`.

### `build.rs` (~350 lines) — `.sam` Builder
Builds `.sam` packages from local files. Creates manifest, signs with Ed25519, zstd-compresses payload.

### `hooks.rs` (~165 lines) — Hook Scripts
Manages pre/post transaction hook scripts (preinst, postinst, prerm, postrm).

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
- `unshare(CLONE_NEWPID)` — child becomes PID 1
- `unshare(CLONE_NEWNS)` + `MS_PRIVATE|MS_REC` — private mount tree
- `mount("tmpfs", "/tmp", "tmpfs")` — isolated temp
- `MS_REMOUNT|MS_RDONLY` — read-only root
- `unshare(CLONE_NEWNET)` — no network
- `unshare(CLONE_NEWUTS)` + `sethostname("sandbox")` — hide hostname

### `seccomp.rs` — BPF Syscall Filter
Allowlist of ~50 safe syscalls. Blocks raw I/O, module loading, kexec, profiling. Architecture support: x86_64, aarch64, arm, i686, riscv64.

### `landlock.rs` — Landlock LSM Rules
Read-only: `/usr/lib`, `/usr/share`. Read-write: sandboxed package directory. Uses `libc::c_long` for syscall constants (works on i686).

### `cgroups.rs` — Cgroup v2 Resource Limits
CPU quota, memory max, IO weight. Falls back gracefully if unavailable.

### `launcher.rs` — Process Launcher
Forks child in new namespaces, applies seccomp + landlock + cgroups before `execvp`.

---

## 8. Content-Addressed Store — `src/store/`

### `deduplication.rs` — Core Dedup
Files stored by BLAKE3 hash, sharded by first 2 hex chars: `store/{origin}/{shard}/{hash}/file`. Same content → same hash → zero extra disk space.

### `hardlink.rs` — Hardlink Management
Hardlinks store paths to target directories. Maintains refcounts in SQLite.

### `gc.rs` — Garbage Collection
Scans store for unreferenced hashes. Dry-run mode (`--dry-run`).

---

## 9. Daemon — `src/daemon/`

### `mod.rs` (~1740 lines) — Daemon Server

Tokio-based UNIX socket server at `/run/spm.sock`:

- **`SO_PEERCRED`** kernel-level authentication (no token, no password)
- **Rate limiter**: max 10 req/s per user
- **Concurrency limiter**: max 3 concurrent transactions per user
- **Signal handling**: SIGINT/SIGTERM → graceful shutdown; SIGHUP → ignored
- **Progress streaming**: handlers send progress via `mpsc::channel`, relay thread writes to socket as `{"type":"progress","message":"..."}`

**28 handlers** (all operations dispatched here):

| Action | Handler |
|--------|---------|
| `install` | `handle_system_install` / `handle_user_install` |
| `remove` | `handle_system_remove` / `handle_user_remove` |
| `purge` | `handle_purge` |
| `autoremove` | `handle_autoremove` |
| `update` | `handle_update` |
| `upgrade` | `handle_upgrade` |
| `dist-upgrade` | `handle_dist_upgrade` |
| `list` | `handle_list` |
| `search` | `handle_search` |
| `search-file` | `handle_search_file` |
| `info` | `handle_info` |
| `files` | `handle_files` |
| `depends` | `handle_depends` |
| `rdepends` | `handle_rdepends` |
| `history` | `handle_history` |
| `snapshot` | `handle_snapshot` |
| `cleanup` | `handle_cleanup` |
| `repo` | `handle_repo` |
| `repo-list` | `handle_repo_list` |
| `repo-create` | `handle_repo_create` |
| `repo-publish` | `handle_repo_publish` |
| `repo-gen-key` | `handle_repo_gen_key` |
| `repo-sign` | `handle_repo_sign` |
| `config-show` | `handle_config_show` |
| `config-set` | `handle_config_set` |
| `index-rebuild` | `handle_index_rebuild` |
| `init` | `handle_init` |
| `group` | `handle_group` |
| `fsck` | `handle_fsck` |
| `sync` | `handle_sync` |
| `sandbox-list` | `handle_sandbox_list` |
| `sandbox-run` | `handle_sandbox_run` |
| `install-local` | `handle_install_local` |
| `install-sandbox` | `handle_install_sandbox` |
| `build` | `handle_build` |
| `convert` | `handle_convert` |
| `detect` | `handle_detect` |
| `analyze` | `handle_analyze` |
| `ps` | `handle_ps` |
| `self-update` | `handle_self_update` |

### `ipc.rs` — IPC Protocol
- `ClientRequest`: action + package + params + user_id + user_home
- `DaemonResponse`: status + message + optional data
- `send_command(action, params)` → serialises request → socket → deserialises response

### `permission.rs` — Permission Engine
- `PeerCreds`: UID/PID/GID from `SO_PEERCRED`
- `is_authorized()`: checks membership in `spm` UNIX group via `getgrouplist(3)` + primary group check
- `check_permission()`: validates file access and operations

### `transaction.rs` — Transaction Manager
6-phase atomic install: Fetch → Detect conflicts → Prepare store → Pre-install scripts → DB commit → Post-install scripts. `RollbackGuard` on panic.

### `conflict.rs` — File Conflict Detection
HashSet intersection for file-level conflict detection. Severity classification (critical/shared/minor).

### `monitor.rs` — Transaction Monitoring
Tracks active transactions per user.

### `store.rs` — Daemon Store Helpers
Store path resolution for the daemon.

---

## 10. Analysis — `src/analyze/mod.rs` (~420 lines)

- `full_analysis()`: runs orphans + conflicts + dependency cycles
- `find_orphans()`: packages with no dependents
- `find_conflicts()`: file-level conflict detection
- `find_dependency_cycles()`: DFS with visited + recursion stack
- `trace_binary()`: runs `ldd` → suggests missing libraries

---

## 11. Isolation — `src/isolation/`

### `detector.rs` — PackageManagerDetector
Probes for dpkg/rpm/pacman on the system. Watches for foreign invocations via `inotify`.

### `freezer.rs` — ProcessFreezer
`SIGSTOP` / `SIGCONT` for process suspension. `SIGKILL` for forceful termination.

### `mediator.rs` — ConflictMediator
Detects file conflicts between SPM and foreign packages. Resolution options: Allow, Deny, Sandbox.

---

## 12. User Experience — `src/ux/`

### `progress.rs` — SpmProgress
`MultiProgress` via `indicatif`. `progress_bar()`, `spinner()`, `set_message()`, `finish_with_message()`.

### `prompts.rs` — SpmPrompts
`confirm_install()`: package list confirmation. `select_conflict_resolution()`: conflict picker. Uses `dialoguer` with coloured themes.

### `formatter.rs` — SpmFormatter
`print_install_summary()`: formatted install results. `print_error_actionable()`: error with fix suggestions. Coloured output via `colored` crate.

### `completions.rs` — Shell Completions
Generates bash/zsh/fish completion scripts via `clap_complete`.

---

## 13. Backend Management — `src/backend/mod.rs` (~160 lines)

### Backend Resolution Order
```
resolve("dnf"):
  1. /var/lib/spm/store/backend/dnf/bin/dnf    ← managed copy
  2. /usr/libexec/spm/backend/dnf               ← bundled with SPM
  3. /dev/null/spm-backend-missing              ← fail cleanly (never system PATH)
```

- `check_missing()`: detects which backends are required; displays warning once per process via `OnceLock`
- `download_backend()`: downloads backend binary from URL

---

## 14. Utilities — `src/util/`

- `hash.rs`: `hash_file()` / `hash_bytes()` (BLAKE3), `sha256_hex()`
- `process.rs`: `find_deleted_libs()` scans `/proc/*/maps` for `(deleted)` shared libraries
- `backend.rs`: backend binary resolution (never system PATH)
- `fs.rs`: atomic write + lock file helpers
- `user.rs`: `resolve_user_name(uid)`

---

## 15. Verification — `src/verify.rs` (~135 lines)
- `verify_manifest_signature()`: Ed25519 signature verification for `.sam` manifests
- `prompt_before_install()`: user confirmation prompt

---

## 16. Terminal Output — `src/output.rs` (~545 lines)
- `section()`, `step_info()`, `step_success()`, `step_warn()`, `step_error()`: structured output helpers with progress channel integration
- `bold()`, `dim()`, `green()`, `red()`, `yellow()`, `blue()`, `cyan()`, `magenta()`: terminal styling (respects `NO_COLOR`)
- `result_summary()`, `result_message()`, `remove_message()`, `show_installed_info()`: formatted result display
- `fmt_duration()`, `fmt_size()`, `fmt_speed()`: formatting helpers
- `Spinner`: lightweight spinner for subprocess output
- `ProgressBar`: interactive download progress bar

---

## Data Flow: `spm install curl`

```
1. cli/args.rs → client::send_command("install", json!({
       "package": "curl", "yes": true, ...}))

2. client.rs → JSON over Unix socket → spmd

3. daemon/mod.rs → handle_system_install
   a. resolve_strategy(req) → VersionStrategy
   b. install_package("curl", None, false, true, false, strategy)
      → install_package_smart:
        - load_repos()
        - find candidates for "curl"
        - resolve_dependencies → ResolvedGraph
        - TransactionEngine::plan_install → TransactionPlan
        - display_plan → approve_plan
        - TransactionEngine::execute (write lock):
          - fetch_and_extract (HTTP from mirror)
          - verify (SHA256 or Ed25519)
          - run preinst scripts
          - hardlink files from store to target
          - record files + transaction in SQLite
          - run postinst scripts

4. DaemonResponse { status: "ok", message: "Installed curl (system)" } → socket
```

On error:
- Package not found → `SpmError::PackageNotFound`
- Already installed → `SpmError::PackageAlreadyInstalled`
- Network failure → `SpmError::Network`
- → `DaemonResponse { status: "error", message: "..." }` → CLI prints error

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
