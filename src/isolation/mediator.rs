use std::path::Path;

use crate::error::{SpmError, SpmResult};
use crate::store::StoreManager;

use super::detector::{ForeignPackageInfo, PackageManager, PackageManagerDetector};
use super::freezer::ProcessFreezer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    None,
    Low,
    High,
}

#[derive(Debug, Clone)]
pub enum ResolutionOption {
    UserRemovesForeign {
        description: String,
        command: String,
        risk: RiskLevel,
    },
    SandboxInstall {
        description: String,
        risk: RiskLevel,
    },
    ForceReplace {
        description: String,
        risk: RiskLevel,
    },
    Cancel {
        description: String,
        risk: RiskLevel,
    },
}

impl ResolutionOption {
    pub fn description(&self) -> &str {
        match self {
            ResolutionOption::UserRemovesForeign { description, .. } => description,
            ResolutionOption::SandboxInstall { description, .. } => description,
            ResolutionOption::ForceReplace { description, .. } => description,
            ResolutionOption::Cancel { description, .. } => description,
        }
    }

    pub fn risk(&self) -> RiskLevel {
        match self {
            ResolutionOption::UserRemovesForeign { risk, .. } => *risk,
            ResolutionOption::SandboxInstall { risk, .. } => *risk,
            ResolutionOption::ForceReplace { risk, .. } => *risk,
            ResolutionOption::Cancel { risk, .. } => *risk,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Proceed,
    SandboxInstall,
    Cancelled,
}

pub struct ConflictMediator {
    pub detector: PackageManagerDetector,
    pub freezer: ProcessFreezer,
    pub store: StoreManager,
}

impl ConflictMediator {
    pub fn new(
        detector: PackageManagerDetector,
        freezer: ProcessFreezer,
        store: StoreManager,
    ) -> Self {
        Self {
            detector,
            freezer,
            store,
        }
    }

    pub async fn handle_foreign_owned(
        &self,
        pkg_name: &str,
        foreign_mgr: PackageManager,
    ) -> SpmResult<Resolution> {
        let foreign_info = self.detector.get_package_info(pkg_name, foreign_mgr)?;

        let options = self.build_foreign_owned_options(pkg_name, foreign_mgr);

        Self::show_conflict_ui(pkg_name, foreign_mgr, &foreign_info, &options);

        let choice = self.prompt_user_choice(&options)?;

        match choice {
            ResolutionOption::UserRemovesForeign { command, .. } => {
                eprintln!(
                    "Please run the following command manually:\n  {}\nThen press Enter to continue...",
                    command
                );
                let mut input = String::new();
                let _ = std::io::stdin().read_line(&mut input);

                if self.detector.is_package_installed(pkg_name, foreign_mgr)? {
                    return Err(SpmError::other(format!(
                        "{pkg_name} is still installed via {}",
                        foreign_mgr.cli_name()
                    )));
                }
                Ok(Resolution::Proceed)
            }
            ResolutionOption::SandboxInstall { .. } => Ok(Resolution::SandboxInstall),
            ResolutionOption::ForceReplace { .. } => {
                Ok(Resolution::Proceed)
            }
            ResolutionOption::Cancel { .. } => Ok(Resolution::Cancelled),
        }
    }

    pub async fn handle_foreign_touch_spm_file(
        &self,
        path: &Path,
        foreign_mgr: PackageManager,
        pid: u32,
    ) -> SpmResult<()> {
        ProcessFreezer::freeze(pid)?;

        let spm_pkg = find_spm_package_owning_file(path)?;

        eprintln!(
            "Warning: foreign package manager '{}' (PID {}) is modifying SPM files.\n\
             File: {}\n\
             Owned by SPM package: {}",
            foreign_mgr.cli_name(),
            pid,
            path.display(),
            spm_pkg.as_deref().unwrap_or("unknown"),
        );

        let options = vec![
            format!(
                "Allow modification (remove SPM protection for this file and let {} proceed)",
                foreign_mgr.cli_name()
            ),
            format!("Deny modification (kill process {pid})"),
            "Install the package in an isolated Sandbox instead".to_string(),
        ];

        let choice = Self::prompt_select(&options);

        match choice {
            0 => {
                ProcessFreezer::unfreeze(pid)?;
                eprintln!("Modification permitted.");
            }
            1 => {
                ProcessFreezer::kill(pid, true)?;
                eprintln!("Foreign process killed.");
            }
            2 => {
                ProcessFreezer::kill(pid, true)?;
                if let Some(pkg) = spm_pkg {
                    eprintln!("Run: spm install --sandbox {pkg}");
                }
            }
            _ => {
                ProcessFreezer::unfreeze(pid)?;
            }
        }

        Ok(())
    }

    fn build_foreign_owned_options(
        &self,
        pkg_name: &str,
        foreign_mgr: PackageManager,
    ) -> Vec<ResolutionOption> {
        vec![
            ResolutionOption::UserRemovesForeign {
                description: format!(
                    "Remove '{}' manually via {} remove {}",
                    pkg_name,
                    foreign_mgr.cli_name(),
                    pkg_name
                ),
                command: format!("{} remove {}", foreign_mgr.cli_name(), pkg_name),
                risk: RiskLevel::Low,
            },
            ResolutionOption::SandboxInstall {
                description: format!("Install '{}' in an isolated Sandbox", pkg_name),
                risk: RiskLevel::None,
            },
            ResolutionOption::ForceReplace {
                description: format!(
                    "Remove the foreign package and install SPM version (may break other applications)"
                ),
                risk: RiskLevel::High,
            },
            ResolutionOption::Cancel {
                description: "Cancel installation".to_string(),
                risk: RiskLevel::None,
            },
        ]
    }

    fn show_conflict_ui(
        pkg_name: &str,
        foreign_mgr: PackageManager,
        info: &ForeignPackageInfo,
        options: &[ResolutionOption],
    ) {
        eprintln!(
            "\nConflict detected: '{pkg_name}' is already managed by {} (version {}) - {} files tracked",
            foreign_mgr.cli_name(),
            info.version,
            info.files_count,
        );
        eprintln!("Select a resolution:\n");
        for (i, opt) in options.iter().enumerate() {
            let risk_tag = match opt.risk() {
                RiskLevel::None => "",
                RiskLevel::Low => " [low risk]",
                RiskLevel::High => " [HIGH RISK]",
            };
            eprintln!("  {}. {}{}", i + 1, opt.description(), risk_tag);
        }
    }

    fn prompt_user_choice(&self, options: &[ResolutionOption]) -> SpmResult<ResolutionOption> {
        eprint!("Enter choice (1-{}): ", options.len());
        let mut input = String::new();
        std::io::stdin()
            .read_line(&mut input)
            .map_err(|e| SpmError::other(format!("failed to read input: {e}")))?;
        let idx: usize = input
            .trim()
            .parse()
            .map_err(|_| SpmError::other("invalid input: enter a number"))?;
        if idx == 0 || idx > options.len() {
            return Err(SpmError::other(format!(
                "choice must be between 1 and {}",
                options.len()
            )));
        }
        Ok(options[idx - 1].clone())
    }

    fn prompt_select(options: &[String]) -> usize {
        eprintln!("\nOptions:");
        for (i, opt) in options.iter().enumerate() {
            eprintln!("  {}. {}", i + 1, opt);
        }
        eprint!("Enter choice (1-{}): ", options.len());
        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_ok() {
            if let Ok(n) = input.trim().parse::<usize>() {
                if n >= 1 && n <= options.len() {
                    return n - 1;
                }
            }
        }
        0
    }
}

fn find_spm_package_owning_file(path: &Path) -> SpmResult<Option<String>> {
    let path_str = path.to_string_lossy();
    let conn = crate::db::get_connection()?;
    let packages = crate::db::files::get_packages_for_file(&conn, path_str.as_ref())?;
    Ok(packages.into_iter().next())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::StoreManager;

    fn make_detector() -> PackageManagerDetector {
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        PackageManagerDetector::new(tx).unwrap()
    }

    fn make_store() -> StoreManager {
        let dir = tempfile::tempdir().unwrap();
        StoreManager::new(dir.path().to_path_buf())
    }

    #[test]
    fn test_resolution_option_description() {
        let opt = ResolutionOption::Cancel {
            description: "Cancel installation".into(),
            risk: RiskLevel::None,
        };
        assert_eq!(opt.description(), "Cancel installation");
    }

    #[test]
    fn test_resolution_option_risk() {
        let opt = ResolutionOption::ForceReplace {
            description: "replace".into(),
            risk: RiskLevel::High,
        };
        assert_eq!(opt.risk(), RiskLevel::High);
    }

    #[test]
    fn test_resolution_equality() {
        assert_eq!(Resolution::Proceed, Resolution::Proceed);
        assert_eq!(Resolution::Cancelled, Resolution::Cancelled);
        assert_ne!(Resolution::Proceed, Resolution::Cancelled);
    }

    #[test]
    fn test_risk_level_equality() {
        assert_eq!(RiskLevel::None, RiskLevel::None);
        assert_ne!(RiskLevel::Low, RiskLevel::High);
    }

    #[test]
    fn test_build_foreign_owned_options() {
        let detector = make_detector();
        let freezer = ProcessFreezer::new();
        let store = make_store();
        let mediator = ConflictMediator::new(detector, freezer, store);

        let options = mediator.build_foreign_owned_options("nginx", PackageManager::Apt);
        assert_eq!(options.len(), 4);

        assert!(matches!(
            options[0],
            ResolutionOption::UserRemovesForeign { .. }
        ));
        assert!(matches!(
            options[1],
            ResolutionOption::SandboxInstall { .. }
        ));
        assert!(matches!(
            options[2],
            ResolutionOption::ForceReplace { .. }
        ));
        assert!(matches!(options[3], ResolutionOption::Cancel { .. }));
    }

    #[test]
    fn test_show_conflict_ui_does_not_panic() {
        let info = ForeignPackageInfo {
            name: "nginx".into(),
            manager: PackageManager::Apt,
            version: "1.24.0".into(),
            files_count: 42,
        };
        let options = vec![
            ResolutionOption::Cancel {
                description: "cancel".into(),
                risk: RiskLevel::None,
            },
        ];
        ConflictMediator::show_conflict_ui("nginx", PackageManager::Apt, &info, &options);
    }

    #[test]
    fn test_new_mediator() {
        let detector = make_detector();
        let freezer = ProcessFreezer::new();
        let store = make_store();
        let mediator = ConflictMediator::new(detector, freezer, store);
        assert!(format!("{:?}", mediator.detector.watched_paths()).contains('/'));
    }

    #[test]
    fn test_find_spm_package_owning_file_no_db() {
        let path = Path::new("/nonexistent/file.so");
        let result = find_spm_package_owning_file(path);
        assert!(result.is_err() || result.unwrap().is_none());
    }
}
