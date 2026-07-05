use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{SpmError, SpmResult};
use crate::integration;

fn system_apps_dir() -> PathBuf {
    if unsafe { libc::geteuid() } == 0 {
        PathBuf::from("/usr/share/applications")
    } else {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        PathBuf::from(home).join(".local/share/applications")
    }
}

fn wrapper_name(sandbox_name: &str, desktop_basename: &str) -> String {
    format!("spm-sandbox-{}-{}", sandbox_name, desktop_basename)
}

fn find_icon_path(sandbox_dir: &Path, icon_name: &str) -> Option<String> {
    if icon_name.starts_with('/') {
        let candidate = Path::new(icon_name);
        if candidate.exists() {
            return Some(icon_name.to_string());
        }
        return None;
    }

    let icon_extensions = [".png", ".svg", ".xpm", ".svgz"];
    let icon_dirs = [
        sandbox_dir.join("usr/share/icons/hicolor/256x256/apps"),
        sandbox_dir.join("usr/share/icons/hicolor/128x128/apps"),
        sandbox_dir.join("usr/share/icons/hicolor/64x64/apps"),
        sandbox_dir.join("usr/share/icons/hicolor/48x48/apps"),
        sandbox_dir.join("usr/share/icons/hicolor/32x32/apps"),
        sandbox_dir.join("usr/share/icons/hicolor/scalable/apps"),
        sandbox_dir.join("usr/share/icons/hicolor/16x16/apps"),
        sandbox_dir.join("usr/share/icons"),
        sandbox_dir.join("usr/share/pixmaps"),
    ];

    for dir in &icon_dirs {
        if !dir.exists() {
            continue;
        }
        for ext in &icon_extensions {
            let candidate = dir.join(format!("{}{}", icon_name, ext));
            if candidate.exists() {
                return candidate.to_str().map(|s| s.to_string());
            }
        }
        let without_ext = dir.join(icon_name);
        if without_ext.exists() {
            return without_ext.to_str().map(|s| s.to_string());
        }
    }

    None
}

fn strip_exec_path(exec_value: &str) -> String {
    let trimmed = exec_value.trim();
    let parts: Vec<&str> = trimmed.splitn(2, |c: char| c.is_whitespace()).collect();
    let binary = parts[0];
    let rest = parts.get(1).copied().unwrap_or("");

    let bin_name = Path::new(binary)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| binary.to_string());

    if rest.is_empty() {
        bin_name
    } else {
        format!("{} {}", bin_name, rest)
    }
}

pub fn create_sandbox_desktop_entries(sandbox_name: &str, sandbox_dir: &Path) -> SpmResult<()> {
    let apps_source = sandbox_dir.join("usr/share/applications");
    if !apps_source.exists() {
        return Ok(());
    }

    let spm_bin = std::env::current_exe()
        .map_err(|e| SpmError::other(format!("Cannot determine spm path: {e}")))?;
    let spm_bin_str = spm_bin.to_string_lossy().to_string();

    let apps_target = system_apps_dir();
    fs::create_dir_all(&apps_target)?;

    let mut created_count = 0;

    let entries = match fs::read_dir(&apps_source) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
            continue;
        }
        if !path.is_file() {
            continue;
        }

        let dest_basename = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        let dest_name = wrapper_name(sandbox_name, dest_basename);
        let dest_path = apps_target.join(&dest_name);

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Cannot read {}: {e}", path.display());
                continue;
            }
        };

        let mut out = Vec::new();
        let mut has_exec = false;

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') || trimmed.is_empty() {
                out.push(line.to_string());
                continue;
            }

            if let Some(value) = trimmed.strip_prefix("Exec=") {
                has_exec = true;
                let modified_exec = strip_exec_path(value);
                let quoted_spm = if spm_bin_str.contains(' ') {
                    format!("\"{}\"", spm_bin_str)
                } else {
                    spm_bin_str.clone()
                };
                out.push(format!("Exec={} sandbox run {} {}", quoted_spm, sandbox_name, modified_exec));
            } else if let Some(icon_value) = trimmed.strip_prefix("Icon=") {
                let icon = icon_value.trim();
                if let Some(abs_path) = find_icon_path(sandbox_dir, icon) {
                    let quoted = if abs_path.contains(' ') {
                        format!("\"{}\"", abs_path)
                    } else {
                        abs_path
                    };
                    out.push(format!("Icon={}", quoted));
                } else {
                    out.push(line.to_string());
                }
            } else if trimmed.starts_with("TryExec=") {
                out.push(format!("TryExec={}", spm_bin_str));
            } else {
                out.push(line.to_string());
            }
        }

        if !has_exec {
            tracing::debug!("No Exec= in {}, skipping", path.display());
            continue;
        }

        let output = out.join("\n");
        if let Err(e) = fs::write(&dest_path, output.as_bytes()) {
            tracing::warn!("Cannot write {}: {e}", dest_path.display());
            continue;
        }

        created_count += 1;
    }

    if created_count > 0 {
        let _ = integration::desktop::update_desktop_database(&apps_target);
        crate::output::step_info(format!(
            "Created {} desktop entr{} for sandbox '{}'",
            created_count,
            if created_count == 1 { "y" } else { "ies" },
            sandbox_name,
        ));
    }

    Ok(())
}

pub fn remove_sandbox_desktop_entries(sandbox_name: &str) -> SpmResult<()> {
    let apps_dir = system_apps_dir();
    if !apps_dir.exists() {
        return Ok(());
    }

    let prefix = format!("spm-sandbox-{}-", sandbox_name);
    let mut removed_count = 0;

    let entries = match fs::read_dir(&apps_dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let fname = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if !fname.starts_with(&prefix) || !fname.ends_with(".desktop") {
            continue;
        }
        if let Err(e) = fs::remove_file(&path) {
            tracing::warn!("Cannot remove {}: {e}", path.display());
        } else {
            removed_count += 1;
        }
    }

    if removed_count > 0 {
        let _ = integration::desktop::update_desktop_database(&apps_dir);
        tracing::debug!(
            "Removed {} desktop entr{} for sandbox '{}'",
            removed_count,
            if removed_count == 1 { "y" } else { "ies" },
            sandbox_name,
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_exec_path_relative() {
        assert_eq!(strip_exec_path("steam %u"), "steam %u");
    }

    #[test]
    fn test_strip_exec_path_absolute() {
        assert_eq!(strip_exec_path("/usr/bin/steam %u"), "steam %u");
    }

    #[test]
    fn test_strip_exec_path_no_args() {
        assert_eq!(strip_exec_path("/opt/steam/steam.sh"), "steam.sh");
    }

    #[test]
    fn test_strip_exec_path_with_flags() {
        assert_eq!(
            strip_exec_path("/usr/bin/firefox --new-window %u"),
            "firefox --new-window %u"
        );
    }

    #[test]
    fn test_strip_exec_path_bare() {
        assert_eq!(strip_exec_path("steam"), "steam");
    }

    #[test]
    fn test_wrapper_name() {
        assert_eq!(
            wrapper_name("steam", "steam.desktop"),
            "spm-sandbox-steam-steam.desktop"
        );
    }

    #[test]
    fn test_system_apps_dir_format() {
        let dir = system_apps_dir();
        assert!(dir.to_string_lossy().ends_with("/applications"));
    }

    #[test]
    fn test_find_icon_path_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let result = find_icon_path(dir.path(), "nonexistent");
        assert!(result.is_none());
    }

    #[test]
    fn test_find_icon_path_in_sandbox() {
        let dir = tempfile::tempdir().unwrap();
        let icon_dir = dir.path().join("usr/share/icons/hicolor/256x256/apps");
        fs::create_dir_all(&icon_dir).unwrap();
        fs::write(icon_dir.join("steam.png"), b"icon").unwrap();

        let result = find_icon_path(dir.path(), "steam");
        assert!(result.is_some());
        assert!(result.unwrap().ends_with("/steam.png"));
    }

    #[test]
    fn test_find_icon_path_svg() {
        let dir = tempfile::tempdir().unwrap();
        let icon_dir = dir.path().join("usr/share/icons/hicolor/scalable/apps");
        fs::create_dir_all(&icon_dir).unwrap();
        fs::write(icon_dir.join("app.svg"), b"icon").unwrap();

        let result = find_icon_path(dir.path(), "app");
        assert!(result.is_some());
        assert!(result.unwrap().ends_with("/app.svg"));
    }

    #[test]
    fn test_remove_sandbox_desktop_no_dir() {
        let result = remove_sandbox_desktop_entries("nonexistent");
        assert!(result.is_ok());
    }
}
