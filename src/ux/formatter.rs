use colored::Colorize;

use crate::error::SpmError;
use crate::output::{fmt_duration, fmt_size};
use crate::ux::InstallResult;

/// Smart output formatting for install summaries and actionable errors.
///
/// All output goes to stdout (summary) or stderr (errors) so it can be
/// captured or piped appropriately.
pub struct SpmFormatter;

impl SpmFormatter {
    /// Pretty-print a completed install result.
    pub fn print_install_summary(result: &InstallResult) {
        println!("\n{}", "═══════════════════════════════".dimmed());
        println!("{} {}", "✔".green().bold(), "Installation complete".bold());
        println!("{}", "═══════════════════════════════".dimmed());

        for pkg in &result.packages {
            println!(
                "\n  {} {} {}",
                "📦".blue(),
                pkg.name.bold(),
                format!("v{}", pkg.version).dimmed()
            );
            println!("     {} {}", "Source:".dimmed(), pkg.source);
            println!("     {} {}", "Size:".dimmed(), fmt_size(pkg.size as f64));
            println!("     {} {}", "Files:".dimmed(), pkg.file_count);
            if pkg.sandboxed {
                println!("     {} {}", "🔒".yellow(), "Isolated in Sandbox".yellow());
            }
        }

        println!(
            "\n  {} {} in {}",
            "📊".blue(),
            format!("{} package(s)", result.packages.len()).bold(),
            fmt_duration(result.duration).dimmed()
        );
        println!(
            "  {} {}",
            "💾".blue(),
            fmt_size(result.total_size as f64).dimmed()
        );
    }

    /// Print an error with actionable suggestions when appropriate.
    pub fn print_error_actionable(error: &SpmError) {
        eprintln!("\n{} {}", "✖".red().bold(), "Error".red().bold());
        eprintln!("{}", "──────────────────".dimmed());
        eprintln!("  {}", error);
        eprintln!("{}", "──────────────────".dimmed());

        match error {
            SpmError::PackageNotFound(pkg) => {
                eprintln!("\n  {} {}", "💡".yellow(), "Suggestions:".bold());
                eprintln!("    • {}", "spm update".cyan());
                eprintln!("    • {}", format!("spm global-search {}", pkg).cyan());
                eprintln!("    • {}", "spm repo list".cyan());
            }
            SpmError::DependencyFailed(_) => {
                eprintln!("\n  {} {}", "💡".yellow(), "Options:".bold());
                eprintln!("    • {}", "spm install --sandbox <package>".cyan());
                eprintln!("    • {}", "spm install --replace <package>".cyan());
            }
            _ => {}
        }
    }
}
