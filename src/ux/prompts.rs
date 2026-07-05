use std::path::Path;

use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Confirm, Select};

use crate::ux::{ForeignTouchAction, PackageInfo, PackageManager, ResolutionOption, RiskLevel};

/// Collection of interactive prompts using dialoguer.
///
/// Every method returns `std::io::Result` so callers can handle
/// situations where stdin/stdout are not available (e.g. piped).
pub struct SpmPrompts;

impl SpmPrompts {
    /// Ask the user to confirm installing a set of packages.
    pub fn confirm_install(packages: &[PackageInfo]) -> Result<bool, dialoguer::Error> {
        let mut theme = ColorfulTheme::default();
        theme.prompt_suffix = console::style(" ".to_string());

        println!("\n{}", "The following packages will be installed:".bold());
        for pkg in packages {
            println!(
                "  {} {} ({})",
                "•".green(),
                pkg.name.bold(),
                pkg.version.dimmed()
            );
        }
        println!();

        Confirm::with_theme(&theme)
            .with_prompt("Continue?")
            .default(true)
            .interact()
    }

    /// Let the user pick how to resolve a conflict with a foreign package manager.
    pub fn select_conflict_resolution(
        pkg_name: &str,
        foreign_mgr: PackageManager,
        options: &[ResolutionOption],
    ) -> Result<usize, dialoguer::Error> {
        let mut theme = ColorfulTheme::default();
        theme.prompt_suffix = console::style(" ".to_string());

        println!(
            "\n{}",
            format!("⚠ Conflict: {} is installed via {}", pkg_name, foreign_mgr)
                .yellow()
                .bold()
        );
        println!("{}", "How would you like to proceed?".bold());

        let items: Vec<String> = options
            .iter()
            .map(|o| {
                let risk_color = match o.risk {
                    RiskLevel::None => "●".green(),
                    RiskLevel::Low => "●".yellow(),
                    RiskLevel::Medium => "●".yellow(),
                    RiskLevel::High => "●".red(),
                };
                format!(
                    "{} {}  {}",
                    risk_color,
                    o.description.bold(),
                    o.command.dimmed().cyan()
                )
            })
            .collect();

        Select::with_theme(&theme)
            .with_prompt("Choose an option")
            .items(&items)
            .default(0)
            .interact()
    }

    /// Ask what to do when an external process tries to modify spm-managed files.
    pub fn select_foreign_touch_action(
        path: &Path,
        spm_pkg: &str,
        foreign_mgr: PackageManager,
        pid: u32,
    ) -> Result<ForeignTouchAction, dialoguer::Error> {
        let mut theme = ColorfulTheme::default();
        theme.prompt_suffix = console::style(" ".to_string());

        println!("\n{}", "⚠ Attempt to modify SPM files".red().bold());
        println!("  File: {}", path.display().to_string().cyan());
        println!("  Package: {}", spm_pkg.green());
        println!("  Process: {} (PID: {})", foreign_mgr, pid);

        let options = vec![
            "Allow modification (remove SPM protection)",
            "Deny modification (kill process)",
            "Install in Sandbox instead",
        ];

        let selection = Select::with_theme(&theme)
            .with_prompt("Action")
            .items(&options)
            .default(1)
            .interact()?;

        Ok(match selection {
            0 => ForeignTouchAction::Allow,
            1 => ForeignTouchAction::DenyAndKill,
            2 => ForeignTouchAction::SandboxInstall,
            _ => ForeignTouchAction::DenyAndKill,
        })
    }
}
