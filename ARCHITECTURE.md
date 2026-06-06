# SPM — Architecture

## Overview

SPM (Samad Package Manager) is a Unix package manager that manages its own
backend binaries (dnf, apt, rpm, dpkg) in complete isolation from the system.
It adds its own management layer (SQLite database, .sam format, transaction
log, sandboxing, analysis) while using the managed backends for package
discovery and download.  The system `PATH` is never consulted for backend
resolution — SPM is the sole package manager visible to the user.

```
            CLI (clap — 25 subcommands)
                │
                ▼
        ┌───────┴───────┐
        │   error.rs    │  SpmError enum + SpmResult<T>
        └───────┬───────┘
                │
        ┌───────┴───────┐
        │   types/      │  Package, Manifest, Transaction, SpmConfig, RepoConfig, Version…
        └───────┬───────┘
                │
        ┌───────┼───────────────┬────────────────┐
        ▼       ▼               ▼                ▼
     db/      config/        package/        util/
     SQLite   repos.d        install          hash
     metadata  TOML          sam              process
     packages/               deb              fs (whoami, is_elf)
     transactions/           rpm              backend
     files/                  fetch/           process
     users/                  resolver/
     mapping/                transaction/
     conflict/               scripts/
                             store (origin-prefixed, cross-distro)
                             hooks, cleanup, solver
                                │
                      ┌────────┴────────┐
                      ▼                 ▼
                  sandbox/          analyze/
                  bwrap             orphan
                                    conflict
                                    trace
```

## 1. `src/bin/spm.rs` — Entry Point (25 lines)

- Initializes `tracing_subscriber`
- Parses `spm::cli::args::SpmArgs` via clap
- Catches `SpmError` and prints user-friendly messages

## 2. `src/bin/spmd.rs` — Daemon Entry Point (24 lines)

- Starts the daemon server loop

## 3. `src/cli/` — Command Layer

### `args.rs` (1079 lines)
Clap derive structs:
- `SpmCommand` (16+ subcommands): Install, Remove, Purge, Update, Upgrade, Search, Info, Files, Depends, Rdepends, History, Snapshot, Analyze, Ps, Sandbox, Config, Repo, Gs
- `AnalyzeMode`: Orphan, Conflicts
- `SandboxAction`: List, Run
- `ConfigAction`: Show, Set
- `RepoAction`: Add, Remove, List

### `group.rs` (394 lines)
- Group management for daemon authorization

### `client.rs` (197 lines)
- CLI client for communicating with the daemon

## 4. `src/error.rs` — Error System (306 lines)

**SpmError** enum with 15+ variants:
- `Io`, `Database`, `Serialization`, `TomlParse`, `TomlSerialize` — technical errors wrapping external libraries
- `Network(String)` — HTTP failure
- `PackageNotFound`, `PackageAlreadyInstalled`, `DependencyFailed`, `InvalidFormat` — package logic
- `PermissionDenied`, `Sandbox`, `Config`, `CommandFailed` — system errors
- `Other(String)` — catch-all

Each variant has a custom error message + `impl From` for external libraries (rusqlite, serde_json, ureq, walkdir, etc.).

`SpmResult<T>` = `Result<T, SpmError>`

`format_user_error()` provides user-friendly translations for common error scenarios.

## 5. `src/types/` — Data Definitions (module, 7 files)

### `package.rs`
- `Package`: name, version, dependencies, conflicts, format (Deb/Rpm/Sam)
- `PackageId`: name + format composite key
- `Dependency`, `DependencySource`, `PackageFormat`
- `Manifest`, `PackageSource`, `AiMetadata`, `PackageSignature`

### `config.rs`
- `RepoConfig`: source (Apt/Dnf/Native), priority, distro, mirrors
- `SpmConfig`: db_path, cache_path, sandbox_path, log_level, auto_snapshot
- `SandboxLevel`: None, Standard, Strict, Full
- `RepoIndex`, `RepoIndexRecord`: native repo index format

### `version.rs`
- `Version`: epoch + version + release with RPM-style comparison
- `rpmvercmp()`: RPM version comparison algorithm

### `db.rs`
- `Transaction`, `TransactionAction`, `TransactionStatus`
- `FileRecord`, `FileAction`
- `InstalledPackage`, `InstallType`

## 6. `src/config/` — Configuration

### `mod.rs` (139 lines)
- `SpmConfig::load()`: reads `/etc/spm/spm.conf` TOML
- `SpmConfig::set(key, value)`: atomically modifies config file (write → rename)

### `repos.rs` (826 lines)
- `load_repos()`: reads `/etc/spm/repos.d/*.list` as TOML files, sorted by `priority`
- `add_repo()`, `remove_repo()`: manage repository definitions
- `update_repos()`: triggers apt/dnf/native repo update
- Each `.list` file represents one repository with source type (Apt/Dnf/Native)

### `paths.rs` (137 lines)
- Centralized path resolution for all SPM directories

### `libmap.rs` (179 lines)
- Cross-distro library name mapping (deb ↔ rpm ↔ soname)

## 7. `src/db/` — Database (SQLite)

### `mod.rs` — Connection management + schema init (re-exports submodules)
- Singleton connection via `OnceLock<Mutex<Connection>>`
- `with_write_lock()` / `with_read_lock()`: file-level read-write locking
- `init_schema()`: creates all tables + indexes + seed data (name_mappings, format_priority)

### `packages.rs` — `installed_packages` table CRUD
- `get_installed_package()`, `add_installed_package()`, `remove_installed_package()`
- `list_installed_packages()`, `get_installed_packages_batch()`
- `get_store_hash()`, `get_all_installed_store_hashes()`

### `transactions.rs` — `transactions` table CRUD
- `record_transaction()`, `get_transaction()`, `list_transactions()`
- `update_transaction_status()`

### `files.rs` — `files` table CRUD
- `record_files()`, `get_files_by_package()`, `get_files_for_transaction()`
- `get_files_by_packages_batch()`, `get_all_files()`

### `users.rs` — `user_installs` table CRUD
- `record_user_install()`, `remove_user_install()`, `list_user_installs()`
- Per-user package management with content-addressed store

### `mapping.rs` — Name mapping + format priority
- `resolve_name_mapping()`: deb → (rpm, soname)
- `resolve_rpm_to_deb()`: rpm → (deb, soname)
- `get_format_priority()`: format → priority

### `conflict.rs` (428 lines)
- `conflict_log` table CRUD
- `detect_file_conflicts()`: HashSet intersection for file-level conflict detection
- `classify_conflicts()`: severity classification (critical/shared/minor)

## 8. `src/package/` — Package Processing

### `install/mod.rs` (420 lines) — Core installer
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

autoremove_packages()
  → find orphaned packages (no dependents in DB)
  → prompt → remove each orphan
  → skips spm itself
```

### `install/local.rs` (199 lines)
- Local file install (`.deb`/`.rpm`/`.sam`)

### `install/sandbox.rs` (424 lines)
- Sandbox installation with symlink farm and bwrap integration

### `install/remove.rs` (275 lines)
- Package removal logic

### `install/upgrade.rs` (201 lines)
- Package upgrade via TransactionEngine

### `install/user.rs` (234 lines)
- Per-user package installation

### `fetch/mod.rs` (1103 lines) — Package fetching
- `FetchedPackage` struct
- `fetch_and_extract()`: dispatches to format-specific fetcher
- `fetch_apt_to_temp()`, `fetch_dnf_to_temp()`, `fetch_native_to_temp()`
- Cross-format fallback (HTTP → apt-get → dnf download)
- SHA256 verification from deb822 metadata
- Signature verification (Ed25519)

### `fetch/download.rs` (231 lines)
- `find_deb_in_cache()`: cached .deb lookup

### `fetch/gs.rs` (371 lines) — Global Search
- RPM-MD repository parsing (repomd.xml, primary.xml)
- `download_rpm_from_repo()`: direct RPM download from mirrors
- Cross-distro global package search

### `fetch/scan.rs` (171 lines)
- Local directory scanning for packages

### `resolver.rs` (1163 lines) — Dependency resolver
- `resolve_dependencies()`: BFS-based dependency resolution (uses PubGrub for core)
- `resolve_with_solver()`: PubGrub SAT solver
- `resolve_with_dnf_repoquery()`: fallback — `dnf repoquery --requires --resolve`
- `resolve_with_apt_simulate()`: fallback — `apt-get --simulate install`
- `fetch_dep_names()`: fetches deps from apt-cache / dnf repoquery
- `find_cycles()`: DFS cycle detection
- `topological_sort()`: Kahn's algorithm
- Cached with TTL (300s) + epoch counter

### `transaction.rs` (777 lines) — TransactionEngine
- `TransactionPlan`: plan data structure
- `TransactionEngine::plan_install()`: builds install plan
- `TransactionEngine::display_plan()`: user-facing plan display
- `TransactionEngine::approve_plan()`: user approval prompt
- `TransactionEngine::execute()`: atomic execution (fetch → preinst → store → symlinks → rpath → postinst → record)
- `TransactionEngine::execute_smart()`: smart mode with RPATH isolation
- `RollbackGuard`: panic-safe rollback via Drop

### Other files:
- `deb.rs` (308 lines): .deb parser (ar archive, deb822 control parser)
- `rpm.rs` (501 lines): .rpm parser (lead, header index, cpio newc)
- `sam.rs` (372 lines): .sam format (SAM1 magic, zstd, manifest)
- `build.rs` (347 lines): .sam build from local files
- `scripts.rs` (520 lines): preinst/postinst script execution with timeout + isolation
  - `run_script()`: executes script with PR_SET_NO_NEW_PRIVS, CAP_BSET drop, rlimit, PID namespace
  - `run_script_in_sandbox()`: chrooted sandbox script execution
  - `save_scripts()` / `load_scripts()` / `remove_scripts()`
- `store.rs` (560 lines): content-addressed store (BLAKE3 hashing)
  - `store_package_dir()`: legacy path `store/{shard}/{hash}`
  - `store_package_dir_for_origin()`: origin-prefixed `store/{deb,rpm,sam}/{shard}/{hash}`
  - `detect_host_format()`: detects dpkg/rpm availability for cross-distro
  - `is_cross_distro(&format)`: true when format differs from host
  - `gc_store_with_origin()` / `remove_store_with_origin()`: origin-aware GC
- `cleanup.rs` (294 lines): orphan files, cache, temp data cleanup
- `query.rs` (580 lines): search, info, files, depends, rdepends, search-file
  - `search_file_owner()`: queries SPM DB files table → falls back to dpkg -S / rpm -qf
- `hooks.rs` (162 lines): hook scripts

## 9. `src/sandbox/mod.rs` (155 lines) — Isolation

- `list_sandboxes()`: lists all sandboxed packages
- `run_in_sandbox()`: pure namespace isolation (no bwrap, no chroot)
  - PID namespace: `CLONE_NEWPID` in parent
  - Mount namespace: `CLONE_NEWNS` + `MS_PRIVATE` + tmpfs on /tmp + read-only root
  - Network namespace: `CLONE_NEWNET` — no external connectivity
  - UTS namespace: `CLONE_NEWUTS` + hostname "sandbox"
  - PATH/LD_LIBRARY_PATH isolation for sandbox binaries

## 10. `src/analyze/mod.rs` (419 lines) — Analysis

- `full_analysis()`: runs orphans + conflicts + dependency cycles
- `find_orphans()`: dependency graph via SQLite → packages with no dependents
- `find_conflicts()`: file-level conflict detection
- `find_dependency_cycles()`: DFS with visited + stack
- `trace_binary()`: `ldd` → missing libraries with apt/dnf suggestions

## 11. `src/backend/` — Backend Management (160 lines)

### `mod.rs` — Backend health checks, store management, download
- `check_missing()`: detects which backends are required for the host
  distribution and whether they're available in store or bundled
- `show_warnings()`: called at the start of every `spm` command if any
  backends are missing; displays once per process via `OnceLock`
- `copy_bundled_to_store()`: copies all backends from
  `/usr/libexec/spm/backend/` to `/var/lib/spm/store/backend/{name}/bin/{name}`
- `download_backend(name, url)`: downloads a backend binary from a URL
  into the store (used when bundled backends are absent)
- `list_store_backends()`: enumerates backends currently in the store

### Resolution order
```
resolve("dnf"):
  1. /var/lib/spm/store/backend/dnf/bin/dnf  ← managed copy
  2. /usr/libexec/spm/backend/dnf             ← bundled with SPM
  3. /dev/null/spm-backend-missing            ← fail cleanly
```

## 12. `src/util/` — Utilities

### `hash.rs` (117 lines)
- `hash_file()`, `hash_bytes()`: BLAKE3 hashing
- `sha256_hex()`: SHA256 hex digest

### `process.rs` (183 lines)
- `find_deleted_libs()`: scans `/proc/*/maps` for "(deleted)" shared libraries
- Maps deleted libs to packages via dpkg/rpm

### `backend.rs` (80 lines)
- Backend binary resolution: store → bundled → fail (never system PATH)
- `resolve(name)`: checks `/var/lib/spm/store/backend/{name}/bin/{name}`,
  then `/usr/libexec/spm/backend/{name}`, returns a safe non-existent
  path if neither exists

### `fs.rs` (61 lines)
- File system utilities (atomic write, lock files)

## 13. `src/daemon/mod.rs` (754 lines) — Daemon

- Tokio-based UNIX socket server at `/run/spm.sock`
- `SO_PEERCRED` kernel-level authentication
- Rate limiter (max 10 req/s per user)
- Concurrency limiter (max 3 concurrent per user)
- Signal handling (SIGINT/SIGTERM shutdown, SIGHUP ignored — via tokio `select!` loop)
- Request handling: install, remove, upgrade, repo management
- Client-server protocol via `DaemonRequest`/`DaemonResponse`

## 14. `src/output.rs` (446 lines) — Output Formatting

- `section()`, `step()`, `step_warn()`, `step_success()`: progress display
- `cyan()`, `dim()`: terminal styling

## 15. `src/verify.rs` (132 lines) — Verification

- `verify_manifest_signature()`: Ed25519 signature verification
- `prompt_before_install()`: user confirmation prompt

## Data Flow: `spm install curl`

```
1. cli/args.rs → install::install_package("curl", None, false, false)
2. install_package_smart:
   a. load_repos() → sorted by priority
   b. Find candidates: repos containing "curl"
   c. For first candidate:
      - resolve_dependencies("curl:deb") → ResolvedGraph
      - TransactionEngine::plan_install → TransactionPlan
      - display_plan → approve_plan
      - TransactionEngine::execute (write lock):
        - fetch_and_extract (apt-get download / HTTP)
        - extract (ar → data.tar.zst → /)
        - run preinst scripts
        - record files in SQLite
        - record transaction in SQLite
        - add to installed_packages
        - run postinst scripts
   d. If fetch fails → try next repo candidate
3. SpmResult::Ok(())
```

In case of error:
- Package not found → `SpmError::PackageNotFound`
- Already installed → `SpmError::PackageAlreadyInstalled`
- Network failure → `SpmError::Network`
- → binary prints user-friendly message via `format_user_error()`
