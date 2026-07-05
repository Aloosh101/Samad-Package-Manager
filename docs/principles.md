# Principles

> A package manager is not a tool. It is a covenant between the user and the system.

---

## 1. Pure Rust, Zero External Debt

SPM extracts `.deb`, `.rpm`, and `.sam` archives using pure Rust parsers. No `dpkg`, no `rpm2cpio`, no `apt-get`, no `dnf`. This eliminates an entire class of dependency-chain failures and makes SPM auditable from a single codebase.

*"A package manager that depends on the very tools it manages has surrendered its independence."*

## 2. Atomic Transactions

Every install, remove, or upgrade is an atomic transaction. If any step fails — download, extraction, script execution, database commit — the entire operation rolls back. The system is never left in an inconsistent state.

*"Either everything happens, or nothing happens. There is no in-between."*

## 3. Security as Architecture, Not Afterthought

- **SO_PEERCRED** kernel-level authentication on every IPC request
- **Seccomp BPF** syscall whitelisting for sandboxed processes
- **Landlock LSM** file system access control
- **Linux namespace isolation** (PID, Mount, Network, UTS)
- **Ed25519** signature verification for packages
- **Cgroup v2** resource limits

*"Security is not a feature. It is the default."*

## 4. Content-Addressed Integrity

Every file stored by SPM is addressed by its BLAKE3 hash. Identical content — whether from `.deb`, `.rpm`, or `.sam` — occupies disk space exactly once. Integrity is verified by the hash, not by trust in the source.

*"The hash does not lie. The source might."*

## 5. Cross-Distro Sovereignty

Users should not be locked into a distribution by its package format. SPM treats `.deb`, `.rpm`, and `.sam` as first-class citizens, enabling cross-distro installs without virtual machines, containers, or hacks.

*"The distribution is a preference, not a prison."*

## 6. Offline-First Lan Distribution

SPM's daemon serves packages over a Unix socket, enabling LAN distribution without a central server, internet connection, or cloud dependency. Repositories can be mirrored, cached, and shared neighbour-to-neighbour.

*"The internet is optional. The LAN is not."*

## 7. Transparency

Every transaction is logged. Every file is tracked. Every dependency resolution is explainable. `spm history` shows not just what happened, but why.

*"A package manager should have no secrets from its administrator."*
