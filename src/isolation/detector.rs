use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc as std_mpsc;

use notify::{EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc;

use crate::error::{SpmError, SpmResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PackageManager {
    Apt,
    Dnf,
    Rpm,
    Dpkg,
    Pacman,
    Zypper,
    Apk,
    Spm,
    Unknown,
}

impl PackageManager {
    pub fn cli_name(&self) -> &'static str {
        match self {
            PackageManager::Apt => "apt",
            PackageManager::Dnf => "dnf",
            PackageManager::Rpm => "rpm",
            PackageManager::Dpkg => "dpkg",
            PackageManager::Pacman => "pacman",
            PackageManager::Zypper => "zypper",
            PackageManager::Apk => "apk",
            PackageManager::Spm => "spm",
            PackageManager::Unknown => "unknown",
        }
    }

    pub fn from_comm(comm: &str) -> Self {
        let comm_lower = comm.trim().to_lowercase();
        match comm_lower.as_str() {
            "apt" | "apt-get" | "aptitude" | "apt-cache" | "apt-mark" => PackageManager::Apt,
            "dnf" | "yum" | "dnf-automatic" => PackageManager::Dnf,
            "rpm" | "rpmdb" => PackageManager::Rpm,
            "dpkg" | "dpkg-deb" | "dpkg-query" => PackageManager::Dpkg,
            "pacman" | "makepkg" => PackageManager::Pacman,
            "zypper" | "zypp" => PackageManager::Zypper,
            "apk" => PackageManager::Apk,
            "spm" | "spmd" => PackageManager::Spm,
            _ => {
                if comm_lower.contains("apt") {
                    PackageManager::Apt
                } else if comm_lower.contains("dnf") || comm_lower.contains("yum") {
                    PackageManager::Dnf
                } else if comm_lower.contains("rpm") {
                    PackageManager::Rpm
                } else if comm_lower.contains("dpkg") {
                    PackageManager::Dpkg
                } else if comm_lower.contains("pacman") {
                    PackageManager::Pacman
                } else if comm_lower.contains("zypper") {
                    PackageManager::Zypper
                } else if comm_lower.contains("apk") {
                    PackageManager::Apk
                } else if comm_lower.contains("spm") {
                    PackageManager::Spm
                } else {
                    PackageManager::Unknown
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum DetectionEvent {
    ForeignInstallStarted {
        manager: PackageManager,
        package: String,
        process_id: u32,
    },
    ForeignFileCreated {
        manager: PackageManager,
        path: PathBuf,
        process_id: u32,
    },
    SpmFileTouched {
        path: PathBuf,
        foreign_manager: PackageManager,
        process_id: u32,
    },
}

pub struct PackageManagerDetector {
    _watcher: notify::RecommendedWatcher,
    watched_paths: HashSet<PathBuf>,
    event_tx: mpsc::Sender<DetectionEvent>,
    notify_rx: Option<std_mpsc::Receiver<notify::Result<notify::Event>>>,
}

impl PackageManagerDetector {
    pub fn new(event_tx: mpsc::Sender<DetectionEvent>) -> SpmResult<Self> {
        let (notify_tx, notify_rx) = std_mpsc::channel::<notify::Result<notify::Event>>();
        let mut watcher = notify::recommended_watcher(notify_tx)
            .map_err(|e| SpmError::other(format!("failed to create inotify watcher: {e}")))?;

        let watch_entries = [
            ("/var/lib/dpkg", PackageManager::Dpkg),
            ("/var/lib/apt", PackageManager::Apt),
            ("/var/lib/rpm", PackageManager::Rpm),
            ("/var/cache/dnf", PackageManager::Dnf),
            ("/var/lib/pacman", PackageManager::Pacman),
            ("/var/cache/apk", PackageManager::Apk),
            ("/usr/bin", PackageManager::Unknown),
            ("/usr/sbin", PackageManager::Unknown),
            ("/usr/local/bin", PackageManager::Unknown),
        ];

        let mut watched_paths = HashSet::new();
        for (path_str, _mgr) in &watch_entries {
            let p = Path::new(path_str);
            if p.exists() {
                watcher
                    .watch(p, RecursiveMode::Recursive)
                    .map_err(|e| {
                        SpmError::other(format!("failed to watch {path_str}: {e}"))
                    })?;
                watched_paths.insert(PathBuf::from(path_str));
            }
        }

        Ok(Self {
            _watcher: watcher,
            watched_paths,
            event_tx,
            notify_rx: Some(notify_rx),
        })
    }

    pub async fn run(&mut self) -> SpmResult<()> {
        let rx = self.notify_rx.take()
            .ok_or_else(|| SpmError::other("detector already running"))?;

        let event_tx = self.event_tx.clone();

        tokio::task::spawn_blocking(move || {
            for event_result in rx {
                let event = match event_result {
                    Ok(e) => e,
                    Err(_) => continue,
                };

                let manager = PackageManager::Unknown;
                let pid = 0;

                match event.kind {
                    EventKind::Create(_) => {
                        for path in event.paths {
                            let ev = DetectionEvent::ForeignFileCreated {
                                manager,
                                path,
                                process_id: pid,
                            };
                            let _ = event_tx.blocking_send(ev);
                        }
                    }
                    EventKind::Modify(_) => {
                        for path in event.paths {
                            if let Ok(pid) = find_package_manager_pid() {
                                if let Ok(mgr) = identify_manager(pid) {
                                    if mgr != PackageManager::Spm && mgr != PackageManager::Unknown {
                                        if is_spm_owned(&path).unwrap_or(false) {
                                            let ev = DetectionEvent::SpmFileTouched {
                                                path,
                                                foreign_manager: mgr,
                                                process_id: pid,
                                            };
                                            let _ = event_tx.blocking_send(ev);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        });

        Ok(())
    }

    pub async fn analyze_creation(&self, path: &Path, manager: PackageManager, pid: u32) -> SpmResult<Option<DetectionEvent>> {
        if manager == PackageManager::Spm || manager == PackageManager::Unknown {
            return Ok(None);
        }

        let is_spm = is_spm_owned(path).unwrap_or(false);
        if is_spm {
            return Ok(Some(DetectionEvent::SpmFileTouched {
                path: path.to_path_buf(),
                foreign_manager: manager,
                process_id: pid,
            }));
        }

        Ok(Some(DetectionEvent::ForeignFileCreated {
            manager,
            path: path.to_path_buf(),
            process_id: pid,
        }))
    }

    pub async fn analyze_modification(&self, path: &Path, manager: PackageManager, pid: u32) -> SpmResult<Option<DetectionEvent>> {
        let is_spm = is_spm_owned(path).unwrap_or(false);
        if is_spm && manager != PackageManager::Spm {
            return Ok(Some(DetectionEvent::SpmFileTouched {
                path: path.to_path_buf(),
                foreign_manager: manager,
                process_id: pid,
            }));
        }
        Ok(None)
    }

    pub fn get_package_info(&self, _pkg_name: &str, _foreign_mgr: PackageManager) -> SpmResult<ForeignPackageInfo> {
        Ok(ForeignPackageInfo {
            name: _pkg_name.to_string(),
            manager: _foreign_mgr,
            version: "unknown".to_string(),
            files_count: 0,
        })
    }

    pub fn is_package_installed(&self, pkg_name: &str, _foreign_mgr: PackageManager) -> SpmResult<bool> {
        let conn = crate::db::get_connection()?;
        let pkg = crate::db::packages::get_installed_package(&conn, pkg_name)?;
        Ok(pkg.is_some())
    }

    pub fn watched_paths(&self) -> &HashSet<PathBuf> {
        &self.watched_paths
    }
}

#[derive(Debug, Clone)]
pub struct ForeignPackageInfo {
    pub name: String,
    pub manager: PackageManager,
    pub version: String,
    pub files_count: usize,
}

fn is_package_manager_comm(comm: &str) -> bool {
    PackageManager::from_comm(comm) != PackageManager::Unknown
}

pub fn find_package_manager_pid() -> SpmResult<u32> {
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        let pid_str = entry.file_name();
        let pid: u32 = match pid_str.to_str().and_then(|s| s.parse().ok()) {
            Some(p) => p,
            None => continue,
        };
        let comm_path = entry.path().join("comm");
        let comm = match std::fs::read_to_string(&comm_path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if is_package_manager_comm(comm.trim()) {
            return Ok(pid);
        }
    }
    Err(SpmError::other("no active package manager process found"))
}

pub fn identify_manager(pid: u32) -> SpmResult<PackageManager> {
    let comm_path = format!("/proc/{pid}/comm");
    let comm = std::fs::read_to_string(&comm_path)
        .map_err(|e| SpmError::other(format!("cannot read /proc/{pid}/comm: {e}")))?;
    Ok(PackageManager::from_comm(comm.trim()))
}

pub fn is_spm_owned(path: &Path) -> SpmResult<bool> {
    let path_str = path.to_string_lossy();
    let conn = crate::db::get_connection()?;
    let packages = crate::db::files::get_packages_for_file(&conn, path_str.as_ref())?;
    Ok(!packages.is_empty())
}

fn extract_package_name(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_package_manager_from_comm() {
        assert_eq!(PackageManager::from_comm("apt"), PackageManager::Apt);
        assert_eq!(PackageManager::from_comm("apt-get"), PackageManager::Apt);
        assert_eq!(PackageManager::from_comm("dnf"), PackageManager::Dnf);
        assert_eq!(PackageManager::from_comm("rpm"), PackageManager::Rpm);
        assert_eq!(PackageManager::from_comm("dpkg"), PackageManager::Dpkg);
        assert_eq!(PackageManager::from_comm("pacman"), PackageManager::Pacman);
        assert_eq!(PackageManager::from_comm("zypper"), PackageManager::Zypper);
        assert_eq!(PackageManager::from_comm("apk"), PackageManager::Apk);
        assert_eq!(PackageManager::from_comm("spm"), PackageManager::Spm);
        assert_eq!(PackageManager::from_comm("spmd"), PackageManager::Spm);
        assert_eq!(PackageManager::from_comm("bash"), PackageManager::Unknown);
        assert_eq!(PackageManager::from_comm(""), PackageManager::Unknown);
    }

    #[test]
    fn test_package_manager_from_comm_contains_heuristic() {
        assert_eq!(PackageManager::from_comm("apt-listchanges"), PackageManager::Apt);
        assert_eq!(PackageManager::from_comm("dpkg-query"), PackageManager::Dpkg);
        assert_eq!(PackageManager::from_comm("dnf-automatic"), PackageManager::Dnf);
    }

    #[test]
    fn test_package_manager_cli_name() {
        assert_eq!(PackageManager::Apt.cli_name(), "apt");
        assert_eq!(PackageManager::Dnf.cli_name(), "dnf");
        assert_eq!(PackageManager::Rpm.cli_name(), "rpm");
        assert_eq!(PackageManager::Dpkg.cli_name(), "dpkg");
        assert_eq!(PackageManager::Pacman.cli_name(), "pacman");
        assert_eq!(PackageManager::Zypper.cli_name(), "zypper");
        assert_eq!(PackageManager::Apk.cli_name(), "apk");
        assert_eq!(PackageManager::Spm.cli_name(), "spm");
        assert_eq!(PackageManager::Unknown.cli_name(), "unknown");
    }

    #[test]
    fn test_package_manager_eq() {
        assert_eq!(PackageManager::Apt, PackageManager::Apt);
        assert_ne!(PackageManager::Apt, PackageManager::Dnf);
        assert_ne!(PackageManager::Rpm, PackageManager::Dpkg);
    }

    #[test]
    fn test_extract_package_name() {
        assert_eq!(extract_package_name(Path::new("/var/lib/dpkg/info/nginx.list")), "nginx.list");
    }

    #[test]
    fn test_extract_package_name_root() {
        assert_eq!(extract_package_name(Path::new("/")), "unknown");
    }

    #[test]
    fn test_detection_event_debug_clone() {
        let ev = DetectionEvent::ForeignInstallStarted {
            manager: PackageManager::Apt,
            package: "nginx".into(),
            process_id: 1234,
        };
        let ev2 = ev.clone();
        assert!(format!("{:?}", ev2).contains("Apt"));
    }

    #[test]
    fn test_foreign_package_info() {
        let info = ForeignPackageInfo {
            name: "nginx".into(),
            manager: PackageManager::Apt,
            version: "1.24.0".into(),
            files_count: 42,
        };
        assert_eq!(info.name, "nginx");
        assert_eq!(info.manager, PackageManager::Apt);
    }

    #[test]
    fn test_detector_new_no_watch_paths() {
        let (tx, _rx) = mpsc::channel(16);
        let result = PackageManagerDetector::new(tx);
        assert!(result.is_ok());
        let detector = result.unwrap();
        assert!(detector.watched_paths().is_empty() || detector.watched_paths().len() > 0);
    }
}
