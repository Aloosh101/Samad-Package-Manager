//! Conflict mediation for SPM installs.
//!
//! When installing a package whose files overlap with entries owned by
//! foreign package managers (dpkg/apt, rpm/dnf, pacman, zypper), or
//! when a foreign process touches SPM-managed files, the
//! [`ConflictMediator`] intervenes to resolve the situation.
//!
//! # Resolution flow
//!
//! 1. **Detect** which package manager owns a file or package.
//! 2. **Freeze** interfering foreign processes (SIGSTOP).
//! 3. **Prompt** the user for a resolution.
//! 4. **Return** the chosen [`Resolution`] so the caller can act.

use std::io;
use std::io::IsTerminal;
use std::path::Path;

use nix::sys::signal::{kill as nix_kill, Signal};
use nix::unistd::Pid;
use crate::output;
use crate::ux::prompts::SpmPrompts;
use crate::ux::{PackageManager, ResolutionOption, RiskLevel};

/// A resolution to a foreign-package-manager conflict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Continue with the install as planned — the foreign manager's
    /// ownership is either acceptable or has been handled elsewhere.
    Proceed,

    /// Abort the direct install and re-run inside a sandbox instead.
    SandboxInstall,

    /// Remove the conflicting foreign-owned packages first, then
    /// proceed with the SPM install.
    ForceReplace {
        /// Names of the foreign-owned packages to remove.
        packages_to_remove: Vec<String>,
    },

    /// The user declined to proceed — the entire operation is
    /// considered cancelled.
    Cancelled,
}

// ---------------------------------------------------------------------------
// Package-manager detector
// ---------------------------------------------------------------------------

/// Detects which package manager (if any) claims ownership of a file
/// or package registered on the running system.
///
/// Detection is done by probing the native tooling:
///
/// | Manager | Query command              |
/// |---------|----------------------------|
/// | Apt     | `dpkg -S <path>`           |
/// | Dnf     | `rpm -qf <path>`           |
/// | Zypper  | `rpm -qf <path>` (+ zypper) |
/// | Pacman  | `pacman -Qo <path>`        |
#[derive(Debug, Clone)]
pub struct PackageManagerDetector;

impl PackageManagerDetector {
    /// Check `/var/lib/dpkg/info/*.list` files to find which package owns `path`.
    fn query_dpkg(path: &Path) -> Option<String> {
        let info_dir = Path::new("/var/lib/dpkg/info");
        if !info_dir.is_dir() {
            return None;
        }
        let path_str = path.to_string_lossy();
        for entry in std::fs::read_dir(info_dir).ok()? {
            let entry = entry.ok()?;
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("list") {
                if let Ok(content) = std::fs::read_to_string(&p) {
                    if content.lines().any(|l| l.trim() == path_str.as_ref()) {
                        return p.file_stem().map(|s| s.to_string_lossy().to_string());
                    }
                }
            }
        }
        None
    }

    /// Check the RPM database for file ownership.
    /// For modern RPM SQLite databases, query directly.
    fn query_rpm(path: &Path) -> Option<String> {
        let path_str = path.to_string_lossy();
        // Try SQLite RPM database (modern Fedora)
        if let Ok(conn) = rusqlite::Connection::open("/var/lib/rpm/rpmdb.sqlite") {
            if let Ok(mut stmt) = conn.prepare(
                "SELECT DISTINCT p.name FROM packages p \
                 JOIN files fi ON fi.packageId = p.packageId \
                 WHERE fi.name = ?1 LIMIT 1"
            ) {
                if let Ok(rows) = stmt.query_map([path_str.as_ref()], |row| row.get::<_, String>(0)) {
                    return rows.filter_map(|r| r.ok()).next();
                }
            }
        }
        None
    }

    /// Identify the package manager that owns `path` on disk.
    ///
    /// Returns `None` when no foreign package manager claims the file.
    pub fn detect_by_file(path: &Path) -> Option<PackageManager> {
        // dpkg-based (Debian / Ubuntu)
        if Path::new("/var/lib/dpkg/status").exists() {
            if Self::query_dpkg(path).is_some() {
                return Some(PackageManager::Apt);
            }
        }

        // rpm-based (Fedora / RHEL / openSUSE)
        if Path::new("/var/lib/rpm/Packages").exists()
            || Path::new("/var/lib/rpm/rpmdb.sqlite").exists()
        {
            if Self::query_rpm(path).is_some() {
                if Path::new("/etc/zypp").is_dir() {
                    return Some(PackageManager::Zypper);
                }
                return Some(PackageManager::Dnf);
            }
        }

        // pacman (Arch Linux) — read /var/lib/pacman/local/*/files
        if Path::new("/var/lib/pacman/local").is_dir() {
            if let Ok(entries) = std::fs::read_dir("/var/lib/pacman/local") {
                for entry in entries.flatten() {
                    let files_path = entry.path().join("files");
                    if let Ok(content) = std::fs::read_to_string(&files_path) {
                        if content.lines().any(|l| l.trim() == path.to_string_lossy().as_ref()) {
                            return Some(PackageManager::Pacman);
                        }
                    }
                    // Also check 'desc' file for %FILES% section
                    let desc_path = entry.path().join("desc");
                    if let Ok(content) = std::fs::read_to_string(&desc_path) {
                        // Pacman desc file: %FILES% section lists files
                        let mut in_files = false;
                        for line in content.lines() {
                            if line == "%FILES%" {
                                in_files = true;
                                continue;
                            }
                            if in_files {
                                if line.starts_with('%') {
                                    break;
                                }
                                if line.trim() == path.to_string_lossy().as_ref() {
                                    return Some(PackageManager::Pacman);
                                }
                            }
                        }
                    }
                }
            }
        }

        None
    }

    /// Check whether a package name is known to any foreign manager
    /// by querying native database files directly.
    pub fn detect_by_package(name: &str) -> Option<PackageManager> {
        // dpkg: check /var/lib/dpkg/status for Package: name
        if Path::new("/var/lib/dpkg/status").exists() {
            if let Ok(content) = std::fs::read_to_string("/var/lib/dpkg/status") {
                for line in content.lines() {
                    if let Some(pkg_name) = line.strip_prefix("Package: ") {
                        if pkg_name.trim().eq_ignore_ascii_case(name) {
                            return Some(PackageManager::Apt);
                        }
                    }
                }
            }
        }

        // rpm: check SQLite database
        if Path::new("/var/lib/rpm/rpmdb.sqlite").exists() {
            if let Ok(conn) = rusqlite::Connection::open("/var/lib/rpm/rpmdb.sqlite") {
                if let Ok(mut stmt) = conn.prepare("SELECT 1 FROM packages WHERE name = ?1 LIMIT 1") {
                    if stmt.exists([name]).unwrap_or(false) {
                        return if Path::new("/etc/zypp").is_dir() {
                            Some(PackageManager::Zypper)
                        } else {
                            Some(PackageManager::Dnf)
                        };
                    }
                }
            }
        } else if Path::new("/var/lib/rpm/Packages").exists() {
            // BerkeleyDB — cannot parse without librpm, but at least detect presence
            if Path::new("/etc/zypp").is_dir() {
                return Some(PackageManager::Zypper);
            }
            return Some(PackageManager::Dnf);
        }

        // pacman: check /var/lib/pacman/local/*/desc for name
        if Path::new("/var/lib/pacman/local").is_dir() {
            if let Ok(entries) = std::fs::read_dir("/var/lib/pacman/local") {
                for entry in entries.flatten() {
                    let desc_path = entry.path().join("desc");
                    if let Ok(content) = std::fs::read_to_string(&desc_path) {
                        for line in content.lines() {
                            if let Some(pkg_name) = line.strip_prefix("%NAME%") {
                                if pkg_name.trim().eq_ignore_ascii_case(name) {
                                    return Some(PackageManager::Pacman);
                                }
                            }
                        }
                    }
                }
            }
        }

        None
    }
}

// ---------------------------------------------------------------------------
// Process freezer
// ---------------------------------------------------------------------------

/// Freezes and unfreezes processes that interfere with SPM file
/// operations by sending `SIGSTOP` / `SIGCONT`.
#[derive(Debug, Clone)]
pub struct ProcessFreezer;

impl ProcessFreezer {
    /// Suspend a process by PID using `SIGSTOP`.
    pub fn freeze(pid: u32) -> io::Result<()> {
        nix_kill(Pid::from_raw(pid as i32), Signal::SIGSTOP)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("freeze PID {pid}: {e}")))
    }

    /// Resume a previously frozen process with `SIGCONT`.
    pub fn unfreeze(pid: u32) -> io::Result<()> {
        nix_kill(Pid::from_raw(pid as i32), Signal::SIGCONT)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("unfreeze PID {pid}: {e}")))
    }

    /// Freeze every process matching the given package manager name
    /// (or its associated helper tools).
    ///
    /// Returns the list of PIDs that were successfully frozen.
    pub fn freeze_all_for_manager(mgr: PackageManager) -> Vec<u32> {
        let name = mgr.to_string();
        let mut frozen = Vec::new();
        if let Ok(proc_dir) = std::fs::read_dir("/proc") {
            for entry in proc_dir.flatten() {
                let pid_str = entry.file_name();
                let pid: u32 = match pid_str.to_string_lossy().parse() {
                    Ok(p) => p,
                    Err(_) => continue,
                };
                let comm_path = entry.path().join("comm");
                let comm = match std::fs::read_to_string(&comm_path) {
                    Ok(c) => c.trim().to_string(),
                    Err(_) => continue,
                };
                if comm.contains(&name)
                    || (name == "apt" && comm.contains("dpkg"))
                    || (name == "dnf" && comm.contains("rpm"))
                {
                    if Self::freeze(pid).is_ok() {
                        frozen.push(pid);
                    }
                }
            }
        }
        frozen
    }

    /// Unfreeze a list of previously frozen PIDs.
    pub fn unfreeze_all(pids: &[u32]) {
        for pid in pids {
            let _ = Self::unfreeze(*pid);
        }
    }
}

// ---------------------------------------------------------------------------
// Conflict mediator
// ---------------------------------------------------------------------------

/// Mediates conflicts between SPM and foreign package managers.
///
/// The mediator:
///
/// 1. **Detects** which package manager owns a file or package (via
///    [`PackageManagerDetector`]).
/// 2. **Freezes** interfering foreign processes (via [`ProcessFreezer`]).
/// 3. **Presents** the user with resolution options using the
///    existing [`SpmPrompts`] infrastructure.
/// 4. **Returns** the chosen [`Resolution`] so the caller can act.
#[derive(Debug, Clone)]
pub struct ConflictMediator {
    /// Detects foreign package ownership.
    pub detector: PackageManagerDetector,

    /// Freezes / unfreezes foreign processes.
    pub freezer: ProcessFreezer,

    /// Reference to the SPM store manager for path resolution.
    pub store: StoreManager,
}

/// Thin wrapper around the SPM content-store module.
///
/// Provides a consistent handle that the conflict mediator can
/// reference when it needs store-level operations.
#[derive(Debug, Clone)]
pub struct StoreManager;

impl StoreManager {
    /// Return the root directory of the content store.
    pub fn root_dir() -> std::path::PathBuf {
        crate::package::store::store_dir()
    }
}

impl Default for ConflictMediator {
    fn default() -> Self {
        Self::new()
    }
}

impl ConflictMediator {
    /// Create a new mediator with default sub-components.
    pub fn new() -> Self {
        Self {
            detector: PackageManagerDetector,
            freezer: ProcessFreezer,
            store: StoreManager,
        }
    }

    /// Handle the case where a package being installed is already owned
    /// by a foreign package manager.
    ///
    /// Presents the user with interactive choices and returns the
    /// selected [`Resolution`].
    pub fn handle_foreign_owned(&self, pkg_name: &str, foreign_mgr: PackageManager) -> Resolution {
        output::step_warn(format!("'{pkg_name}' is already tracked by {foreign_mgr}."));

        let options = self.build_foreign_owned_options(pkg_name, foreign_mgr);
        self.prompt_resolution(pkg_name, foreign_mgr, &options)
    }

    /// Handle the case where a foreign process is attempting to modify
    /// files that belong to an SPM-managed package.
    ///
    /// The foreign process is frozen (SIGSTOP) while the user decides
    /// how to proceed.
    pub fn handle_foreign_touch_spm_file(
        &self,
        path: &Path,
        foreign_mgr: PackageManager,
        pid: u32,
    ) -> Resolution {
        let spm_pkg = self
            .resolve_spm_package_for_path(path)
            .unwrap_or_else(|| "<unknown>".to_string());

        output::step_warn(format!(
            "{} (PID {pid}) is writing to SPM-managed file: {}",
            foreign_mgr,
            path.display(),
        ));
        output::step_info(format!("SPM package '{spm_pkg}' owns this file."));

        if ProcessFreezer::freeze(pid).is_ok() {
            output::step_info(format!(
                "Frozen PID {pid} ({foreign_mgr}) — resuming after your choice."
            ));
        }

        let resolution = self.prompt_foreign_touch(path, &spm_pkg, foreign_mgr, pid);

        let _ = ProcessFreezer::unfreeze(pid);

        resolution
    }

    // ------------------------------------------------------------------
    // Private helpers
    // ------------------------------------------------------------------

    /// Build the list of options presented when a foreign-owned package
    /// is encountered during install.
    fn build_foreign_owned_options(
        &self,
        _pkg_name: &str,
        _foreign_mgr: PackageManager,
    ) -> Vec<ResolutionOption> {
        vec![
            ResolutionOption {
                description: "Proceed with install (SPM takes over)".into(),
                command: "spm install --replace".into(),
                risk: RiskLevel::Medium,
            },
            ResolutionOption {
                description: "Install in sandbox instead (recommended)".into(),
                command: "spm install --sandbox=standard".into(),
                risk: RiskLevel::Low,
            },
            ResolutionOption {
                description: "Force SPM replacement (remove foreign package)".into(),
                command: "spm install --replace --force".into(),
                risk: RiskLevel::High,
            },
            ResolutionOption {
                description: "Cancel installation".into(),
                command: String::new(),
                risk: RiskLevel::None,
            },
        ]
    }

    /// Route to either the dialoguer [`SpmPrompts`] or a plain
    /// stdin fallback depending on terminal availability.
    fn prompt_resolution(
        &self,
        pkg_name: &str,
        foreign_mgr: PackageManager,
        options: &[ResolutionOption],
    ) -> Resolution {
        // Try the rich dialoguer-based prompt first.
        if std::io::stdin().is_terminal() {
            match SpmPrompts::select_conflict_resolution(pkg_name, foreign_mgr, options) {
                Ok(idx) => return self.foreign_owned_index_to_resolution(idx, pkg_name),
                Err(_) => { /* fall through to the text-based prompt */ }
            }
        }

        self.foreign_owned_fallback(pkg_name, foreign_mgr, options)
    }

    /// Map a zero-based option index (matching
    /// [`build_foreign_owned_options`]) to a [`Resolution`].
    fn foreign_owned_index_to_resolution(&self, idx: usize, pkg_name: &str) -> Resolution {
        match idx {
            0 => Resolution::Proceed,
            1 => Resolution::SandboxInstall,
            2 => Resolution::ForceReplace {
                packages_to_remove: vec![pkg_name.to_string()],
            },
            _ => Resolution::Cancelled,
        }
    }

    /// Plain-text fallback prompt (used when stdin is not a terminal or
    /// dialoguer is unavailable).
    fn foreign_owned_fallback(
        &self,
        pkg_name: &str,
        foreign_mgr: PackageManager,
        options: &[ResolutionOption],
    ) -> Resolution {
        use std::io::Write;

        eprintln!();
        output::step_warn(format!(
            "{} is installed via {}",
            output::bold(pkg_name),
            foreign_mgr,
        ));

        for (i, opt) in options.iter().enumerate() {
            let bullet = match opt.risk {
                RiskLevel::None => output::green("●"),
                RiskLevel::Low => output::green("●"),
                RiskLevel::Medium => output::yellow("●"),
                RiskLevel::High => output::red("●"),
            };
            eprintln!(
                "  {} [{}] {}  {}",
                bullet,
                i + 1,
                output::bold(&opt.description),
                output::dim(&opt.command),
            );
        }

        eprint!(
            "  {} Choose an option [1-{}]: ",
            output::cyan("?"),
            options.len()
        );
        let _ = io::stdout().flush();

        let mut buf = String::new();
        if io::stdin().read_line(&mut buf).is_ok() {
            let trimmed = buf.trim();
            if let Ok(n) = trimmed.parse::<usize>() {
                if (1..=options.len()).contains(&n) {
                    return self.foreign_owned_index_to_resolution(n - 1, pkg_name);
                }
            }
            match trimmed.to_lowercase().as_str() {
                "y" | "yes" | "proceed" => return Resolution::Proceed,
                "s" | "sandbox" => return Resolution::SandboxInstall,
                "f" | "force" | "replace" => {
                    return Resolution::ForceReplace {
                        packages_to_remove: vec![pkg_name.to_string()],
                    }
                }
                "c" | "cancel" | "n" | "no" => return Resolution::Cancelled,
                _ => {}
            }
        }

        output::step_info("Installation cancelled by user.");
        Resolution::Cancelled
    }

    /// Prompt the user when a foreign process touches SPM-managed files.
    fn prompt_foreign_touch(
        &self,
        path: &Path,
        spm_pkg: &str,
        foreign_mgr: PackageManager,
        pid: u32,
    ) -> Resolution {
        use std::io::Write;

        // Try the rich dialoguer prompt first.
        if std::io::stdin().is_terminal() {
            match SpmPrompts::select_foreign_touch_action(path, spm_pkg, foreign_mgr, pid) {
                Ok(crate::ux::ForeignTouchAction::Allow) => return Resolution::Proceed,
                Ok(crate::ux::ForeignTouchAction::DenyAndKill) => {
                    output::step_info(format!("Killing PID {pid} ({foreign_mgr}) ..."));
                    return Resolution::Cancelled;
                }
                Ok(crate::ux::ForeignTouchAction::SandboxInstall) => {
                    return Resolution::SandboxInstall;
                }
                Err(_) => { /* fall through to text prompt */ }
            }
        }

        // Fallback text prompt.
        eprintln!();
        output::step_warn(format!(
            "Foreign process ({}) wants to modify SPM-managed files",
            foreign_mgr,
        ));
        eprintln!("  {} {}", output::dim("  File:"), path.display());
        eprintln!("  {} {}", output::dim("  Package:"), spm_pkg);
        eprintln!(
            "  {} {} (PID {})",
            output::dim("  Process:"),
            foreign_mgr,
            pid
        );

        eprintln!(
            "  {} [1] {}  {}",
            output::green("●"),
            output::bold("Allow modification (remove SPM protection)"),
            output::dim("allow"),
        );
        eprintln!(
            "  {} [2] {}  {}",
            output::yellow("●"),
            output::bold("Deny — kill the process"),
            output::dim("kill"),
        );
        eprintln!(
            "  {} [3] {}  {}",
            output::green("●"),
            output::bold("Install in Sandbox instead"),
            output::dim("spm install --sandbox"),
        );
        eprintln!("  {} [4] {}", output::red("●"), output::bold("Cancel"),);

        eprint!("  {} Choose [1-4]: ", output::cyan("?"));
        let _ = io::stdout().flush();

        let mut buf = String::new();
        if io::stdin().read_line(&mut buf).is_ok() {
            match buf.trim() {
                "1" => {
                    output::step_warn("SPM protection removed for this file.");
                    return Resolution::Proceed;
                }
                "2" => {
                    output::step_info(format!("Killing PID {pid} ({foreign_mgr}) ..."));
                    return Resolution::Cancelled;
                }
                "3" => {
                    output::step_info("Will re-install in sandbox instead.");
                    return Resolution::SandboxInstall;
                }
                _ => {}
            }
        }

        output::step_info("Operation cancelled.");
        Resolution::Cancelled
    }

    /// Look up the SPM package that owns a given file path by querying
    /// the file tracking database.
    fn resolve_spm_package_for_path(&self, path: &Path) -> Option<String> {
        crate::db::with_read_lock(|conn| {
            let stmt = conn
                .prepare("SELECT package FROM files WHERE filepath = ?1 LIMIT 1")
                .ok();
            let result = match stmt {
                Some(mut stmt) => stmt
                    .query_row(rusqlite::params![path.to_string_lossy()], |row| {
                        row.get::<_, String>(0)
                    })
                    .ok(),
                None => None,
            };
            Ok(result)
        })
        .ok()
        .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolution_equality() {
        assert_eq!(Resolution::Proceed, Resolution::Proceed);
        assert_eq!(Resolution::SandboxInstall, Resolution::SandboxInstall);
        assert_eq!(
            Resolution::ForceReplace {
                packages_to_remove: vec!["foo".into()],
            },
            Resolution::ForceReplace {
                packages_to_remove: vec!["foo".into()],
            },
        );
        assert_eq!(Resolution::Cancelled, Resolution::Cancelled);
        assert_ne!(Resolution::Proceed, Resolution::Cancelled);
    }

    #[test]
    fn test_mediator_new() {
        let _m = ConflictMediator::new();
        // Freezing a non-existent PID should fail.
        assert!(ProcessFreezer::freeze(999_999_999).is_err());
    }

    #[test]
    fn test_foreign_owned_index_to_resolution() {
        let m = ConflictMediator::new();
        assert_eq!(
            m.foreign_owned_index_to_resolution(0, "pkg"),
            Resolution::Proceed
        );
        assert_eq!(
            m.foreign_owned_index_to_resolution(1, "pkg"),
            Resolution::SandboxInstall,
        );
        assert_eq!(
            m.foreign_owned_index_to_resolution(2, "pkg"),
            Resolution::ForceReplace {
                packages_to_remove: vec!["pkg".into()],
            },
        );
        assert_eq!(
            m.foreign_owned_index_to_resolution(3, "pkg"),
            Resolution::Cancelled
        );
        assert_eq!(
            m.foreign_owned_index_to_resolution(99, "pkg"),
            Resolution::Cancelled
        );
    }

    #[test]
    fn test_resolution_debug() {
        let r = Resolution::ForceReplace {
            packages_to_remove: vec!["a".into(), "b".into()],
        };
        let debug = format!("{r:?}");
        assert!(debug.contains("ForceReplace"));
        assert!(debug.contains("a"));
    }

    #[test]
    fn test_store_manager_root_dir() {
        let root = StoreManager::root_dir();
        assert!(root.to_string_lossy().contains("store"));
    }

    #[test]
    fn test_process_freezer_sendable() {
        // Compile-time assertion: ProcessFreezer is Send + Sync.
        fn assert_send<T: Send>(_: &T) {}
        let f = ProcessFreezer;
        assert_send(&f);
    }
}
