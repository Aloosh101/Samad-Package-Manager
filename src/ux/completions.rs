use std::io::Write;

use clap::Command;
use clap_complete::{generate, Shell};

/// Generate shell completion text for the given shell and write it to `buf`.
pub fn generate_completions(shell: Shell, cmd: &mut Command, buf: &mut impl Write) {
    let name = cmd.get_name().to_string();
    generate(shell, cmd, name, buf);
}

/// Generate shell completions and write them directly to a file.
pub fn generate_completions_to_file(
    shell: Shell,
    cmd: &mut Command,
    path: &std::path::Path,
) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    let name = cmd.get_name().to_string();
    generate(shell, cmd, name, &mut file);
    Ok(())
}

/// Return all shells for which spm can generate completions.
pub fn supported_shells() -> &'static [Shell] {
    &[
        Shell::Bash,
        Shell::Zsh,
        Shell::Fish,
        Shell::PowerShell,
        Shell::Elvish,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Command;

    fn test_command() -> Command {
        Command::new("spm")
            .version("0.2.0")
            .about("Samad Package Manager")
            .subcommand(Command::new("install").about("Install a package"))
            .subcommand(Command::new("remove").about("Remove a package"))
    }

    #[test]
    fn test_generate_bash() {
        let mut cmd = test_command();
        let mut buf = Vec::new();
        generate_completions(Shell::Bash, &mut cmd, &mut buf);
        let output = String::from_utf8_lossy(&buf);
        assert!(output.contains("spm"));
        assert!(!buf.is_empty());
    }

    #[test]
    fn test_generate_zsh() {
        let mut cmd = test_command();
        let mut buf = Vec::new();
        generate_completions(Shell::Zsh, &mut cmd, &mut buf);
        let _output = String::from_utf8_lossy(&buf);
        assert!(!buf.is_empty());
    }

    #[test]
    fn test_generate_fish() {
        let mut cmd = test_command();
        let mut buf = Vec::new();
        generate_completions(Shell::Fish, &mut cmd, &mut buf);
        let _output = String::from_utf8_lossy(&buf);
        assert!(!buf.is_empty());
    }

    #[test]
    fn test_supported_shells_not_empty() {
        let shells = supported_shells();
        assert!(!shells.is_empty());
    }

    #[test]
    fn test_generate_to_temp_file() {
        let mut cmd = test_command();
        let dir = std::env::temp_dir();
        let path = dir.join("_spm_completions_test");
        generate_completions_to_file(Shell::Bash, &mut cmd, &path).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("spm"));
        let _ = std::fs::remove_file(&path);
    }
}
