use std::fs;
use std::path::Path;
use std::process::Command;

use crate::types::Manifest;

/// Run system integration hooks after package installation.
/// Detects file types (libraries, systemd units, desktop files, man pages)
/// and runs the appropriate system commands.
pub fn run_install_hooks(installed_files: &[String]) {
    let has_so = installed_files.iter().any(|f| {
        f.ends_with(".so") || f.contains("/lib/") || f.contains("/lib64/")
    });
    if has_so {
        run_ldconfig();
    }

    let has_systemd = installed_files.iter().any(|f| {
        f.contains("/usr/lib/systemd/system/") || f.contains("/etc/systemd/system/")
    });
    if has_systemd {
        run_systemctl_daemon_reload();
    }

    let has_desktop = installed_files.iter().any(|f| {
        f.ends_with(".desktop") && f.contains("/usr/share/applications/")
    });
    if has_desktop {
        run_update_desktop_database();
    }

    let has_man = installed_files.iter().any(|f| {
        f.contains("/usr/share/man/")
    });
    if has_man {
        run_mandb();
    }
}

/// Run kernel-related hooks after package installation.
/// Detects if any installed package is kernel-related and runs DKMS rebuild,
/// initramfs regeneration, and bootloader update.
pub fn run_kernel_hooks(installed_packages: &[String]) {
    let has_kernel = installed_packages.iter().any(|p| {
        crate::kernel::is_kernel_package(p)
    });
    if !has_kernel {
        return;
    }

    // Call handle_kernel_install for each kernel package
    for pkg in installed_packages {
        if crate::kernel::is_kernel_package(pkg) {
            crate::kernel::handle_kernel_install(pkg);
        }
    }
}

/// Run kernel hooks after package removal
pub fn run_kernel_remove_hooks(removed_packages: &[String]) {
    for pkg in removed_packages {
        if crate::kernel::is_kernel_package(pkg) {
            crate::kernel::handle_kernel_remove(pkg);
        }
    }
}

fn run_ldconfig() {
    match Command::new("/sbin/ldconfig").arg("-X").status() {
        Ok(s) if s.success() => tracing::debug!("ldconfig completed"),
        _ => tracing::warn!("ldconfig failed or not available"),
    }
}

fn run_systemctl_daemon_reload() {
    match Command::new("systemctl").arg("daemon-reload").status() {
        Ok(s) if s.success() => tracing::debug!("systemctl daemon-reload completed"),
        _ => tracing::warn!("systemctl daemon-reload failed or not available"),
    }
}

fn run_update_desktop_database() {
    match Command::new("update-desktop-database").arg("-q").status() {
        Ok(s) if s.success() => tracing::debug!("update-desktop-database completed"),
        _ => tracing::warn!("update-desktop-database failed or not available"),
    }
}

fn run_mandb() {
    match Command::new("mandb").arg("-q").status() {
        Ok(s) if s.success() => tracing::debug!("mandb completed"),
        _ => tracing::warn!("mandb failed or not available"),
    }
}

/// Run SAM v2 post-install hooks: systemd_units, sysusers, tmpfiles.
/// Called after a successful install transaction.
pub fn run_sam_v2_hooks(manifests: &[&Manifest]) {
    for manifest in manifests {
        process_systemd_units(manifest);
        process_sysusers(manifest);
        process_tmpfiles(manifest);
    }
    run_systemd_sysusers();
    run_systemd_tmpfiles();
}

/// Undo SAM v2 hooks on package removal: disable/stop systemd units,
/// remove sysusers.d and tmpfiles.d config files.
pub fn remove_sam_v2_hooks(manifest: &Manifest) {
    disable_systemd_units(manifest);
    remove_sysusers_conf(manifest);
    remove_tmpfiles_conf(manifest);
}

/// Disable and stop systemd units shipped by the package being removed.
fn disable_systemd_units(manifest: &Manifest) {
    for unit in &manifest.systemd_units {
        crate::output::step_info(format!("Stopping systemd unit: {}", unit));
        let _ = Command::new("systemctl")
            .args(["stop", unit])
            .status();
        crate::output::step_info(format!("Disabling systemd unit: {}", unit));
        let _ = Command::new("systemctl")
            .args(["disable", unit])
            .status();
    }
}

/// Remove sysusers.d config for a package being removed.
fn remove_sysusers_conf(manifest: &Manifest) {
    if manifest.sysusers.is_empty() {
        return;
    }
    let filename = format!("50-{}.conf", manifest.name);
    let conf_path = Path::new("/etc/sysusers.d").join(&filename);
    if conf_path.exists() {
        if let Err(e) = fs::remove_file(&conf_path) {
            tracing::warn!("Failed to remove sysusers conf {}: {e}", conf_path.display());
        }
    }
}

/// Remove tmpfiles.d config for a package being removed.
fn remove_tmpfiles_conf(manifest: &Manifest) {
    if manifest.tmpfiles.is_empty() {
        return;
    }
    let filename = format!("{}.conf", manifest.name);
    let conf_path = Path::new("/etc/tmpfiles.d").join(&filename);
    if conf_path.exists() {
        if let Err(e) = fs::remove_file(&conf_path) {
            tracing::warn!("Failed to remove tmpfiles conf {}: {e}", conf_path.display());
        }
    }
}

/// Enable and start systemd units shipped by a package.
fn process_systemd_units(manifest: &Manifest) {
    for unit in &manifest.systemd_units {
        let unit_path = Path::new("/etc/systemd/system").join(unit);
        if !unit_path.exists() {
            let unit_file = format!("/usr/lib/systemd/system/{}", unit);
            if !Path::new(&unit_file).exists() {
                tracing::debug!("systemd unit '{}' not found on disk (may be installed later)", unit);
                continue;
            }
        }
        crate::output::step_info(format!("Enabling systemd unit: {}", unit));
        let _ = Command::new("systemctl")
            .args(["enable", unit])
            .status();
        crate::output::step_info(format!("Starting systemd unit: {}", unit));
        let _ = Command::new("systemctl")
            .args(["start", unit])
            .status();
    }
}

/// Write sysusers.d entries and create system users/groups.
fn process_sysusers(manifest: &Manifest) {
    if manifest.sysusers.is_empty() {
        return;
    }
    let sysusers_dir = Path::new("/etc/sysusers.d");
    let _ = fs::create_dir_all(sysusers_dir);

    let filename = format!("50-{}.conf", manifest.name);
    let mut content = String::new();
    for entry in &manifest.sysusers {
        let t = match entry.entry_type {
            crate::types::SysuserType::User => "u",
            crate::types::SysuserType::Group => "g",
            crate::types::SysuserType::Uuid => "m",
        };
        let id_str = entry.id.as_deref().unwrap_or("-");
        let desc = entry.description.as_deref().unwrap_or("");
        let home = entry.home.as_deref().unwrap_or("-");
        let shell = entry.shell.as_deref().unwrap_or("-");
        content.push_str(&format!("{} {} {} {} {}:{}\n", t, entry.name, id_str, desc, home, shell));
    }

    let conf_path = sysusers_dir.join(&filename);
    if let Err(e) = fs::write(&conf_path, &content) {
        tracing::warn!("Failed to write sysusers conf {}: {e}", conf_path.display());
    }
}

/// Write tmpfiles.d entries.
fn process_tmpfiles(manifest: &Manifest) {
    if manifest.tmpfiles.is_empty() {
        return;
    }
    let tmpfiles_dir = Path::new("/etc/tmpfiles.d");
    let _ = fs::create_dir_all(tmpfiles_dir);

    let filename = format!("{}.conf", manifest.name);
    let mut content = String::new();
    for entry in &manifest.tmpfiles {
        let age = entry.age.as_deref().unwrap_or("-");
        let arg = entry.argument.as_deref().unwrap_or("-");
        content.push_str(&format!("{} {} {} {} {} {} {}\n", entry.path, entry.mode, entry.uid, entry.gid, age, arg, ""));
    }

    let conf_path = tmpfiles_dir.join(&filename);
    if let Err(e) = fs::write(&conf_path, &content) {
        tracing::warn!("Failed to write tmpfiles conf {}: {e}", conf_path.display());
    }
}

fn run_systemd_sysusers() {
    let _ = Command::new("systemd-sysusers").status();
}

fn run_systemd_tmpfiles() {
    let _ = Command::new("systemd-tmpfiles").args(["--create"]).status();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> Vec<String> {
        Vec::new()
    }

    #[test]
    fn test_run_install_hooks_empty() {
        // Should not panic with empty list
        run_install_hooks(&empty());
    }

    #[test]
    fn test_run_install_hooks_with_so() {
        let files = vec!["/usr/lib/libfoo.so".into()];
        run_install_hooks(&files);
        // ldconfig may or may not be available; test is just verifying no crash
    }

    #[test]
    fn test_run_install_hooks_with_systemd() {
        let files = vec!["/usr/lib/systemd/system/foo.service".into()];
        run_install_hooks(&files);
    }

    #[test]
    fn test_run_install_hooks_with_desktop() {
        let files = vec!["/usr/share/applications/foo.desktop".into()];
        run_install_hooks(&files);
    }

    #[test]
    fn test_run_install_hooks_with_man() {
        let files = vec!["/usr/share/man/man1/foo.1".into()];
        run_install_hooks(&files);
    }

    #[test]
    fn test_run_install_hooks_all_triggers() {
        let files = vec![
            "/usr/lib/libfoo.so".into(),
            "/usr/lib/systemd/system/foo.service".into(),
            "/usr/share/applications/foo.desktop".into(),
            "/usr/share/man/man1/foo.1".into(),
        ];
        run_install_hooks(&files);
    }

    #[test]
    fn test_run_kernel_hooks_empty() {
        run_kernel_hooks(&empty());
    }

    #[test]
    fn test_run_kernel_hooks_with_kernel() {
        let pkgs = vec!["kernel-default".into()];
        run_kernel_hooks(&pkgs);
    }

    #[test]
    fn test_run_kernel_hooks_with_nvidia() {
        let pkgs = vec!["nvidia-driver".into()];
        run_kernel_hooks(&pkgs);
    }

    #[test]
    fn test_run_kernel_hooks_with_kmod() {
        let pkgs = vec!["kmod-foo".into()];
        run_kernel_hooks(&pkgs);
    }

    #[test]
    fn test_run_kernel_hooks_non_kernel() {
        let pkgs = vec!["bash".into(), "coreutils".into()];
        run_kernel_hooks(&pkgs);
    }

    // ── SAM v2 cleanup tests ──

    #[test]
    fn test_remove_sysusers_conf_empty() {
        let m = Manifest {
            name: "test-pkg".into(),
            ..Manifest::default()
        };
        // Should not panic or try to remove non-existent file
        remove_sysusers_conf(&m);
    }

    #[test]
    fn test_remove_tmpfiles_conf_empty() {
        let m = Manifest {
            name: "test-pkg".into(),
            ..Manifest::default()
        };
        remove_tmpfiles_conf(&m);
    }

    #[test]
    fn test_remove_sam_v2_hooks_noops_on_empty_manifest() {
        let m = Manifest {
            name: "noop-pkg".into(),
            ..Manifest::default()
        };
        // Should not panic — systemctl commands will fail silently
        remove_sam_v2_hooks(&m);
    }

    #[test]
    fn test_disable_systemd_units_does_not_panic() {
        let m = Manifest {
            name: "test-svc".into(),
            systemd_units: vec!["nonexistent-test.service".into()],
            ..Manifest::default()
        };
        // systemctl stop/disable will fail silently for nonexistent units
        disable_systemd_units(&m);
    }
}
