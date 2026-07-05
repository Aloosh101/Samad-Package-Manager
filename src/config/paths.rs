use std::path::PathBuf;
use std::sync::OnceLock;

use crate::types::SpmConfig;

static CONFIG: OnceLock<SpmConfig> = OnceLock::new();

fn config() -> &'static SpmConfig {
    CONFIG.get_or_init(|| {
        match SpmConfig::load() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to load config: {e}. Using defaults.");
                SpmConfig::default()
            }
        }
    })
}

/// If the `SPM_ROOT` environment variable is set, prepend it to the given
/// path.  This lets callers redirect all SPM paths into an isolated
/// directory tree for E2E testing without sudo.
fn maybe_root(path: PathBuf) -> PathBuf {
    match std::env::var("SPM_ROOT") {
        Ok(root) if !root.is_empty() => PathBuf::from(root).join(path.strip_prefix("/").unwrap_or(&path)),
        _ => path,
    }
}

pub fn db_base() -> PathBuf {
    maybe_root(config().db_path.as_deref().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/var/lib/spm")))
}

fn cache_base() -> PathBuf {
    maybe_root(config().cache_path.as_deref().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/var/cache/spm")))
}

pub fn metadata_db() -> PathBuf {
    db_base().join("metadata.db")
}

pub fn packages_dir() -> PathBuf {
    db_base().join("packages")
}

pub fn sandboxes_dir() -> PathBuf {
    maybe_root(config().sandbox_path.as_deref().map(PathBuf::from).unwrap_or_else(|| db_base().join("sandboxes")))
}

pub fn archives_dir() -> PathBuf {
    cache_base().join("archives")
}

pub fn repos_cache_dir() -> PathBuf {
    cache_base().join("repos")
}

pub fn cache_dir() -> PathBuf {
    cache_base()
}

pub fn repos_config_dir() -> PathBuf {
    maybe_root(PathBuf::from("/etc/spm/repos.d"))
}

pub fn config_file() -> PathBuf {
    maybe_root(PathBuf::from("/etc/spm/spm.conf"))
}

pub fn user_config_file() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        maybe_root(PathBuf::from(dir).join("spm").join("spm.conf"))
    } else if let Ok(home) = std::env::var("HOME") {
        maybe_root(PathBuf::from(home).join(".config").join("spm").join("spm.conf"))
    } else {
        maybe_root(PathBuf::from("/etc/spm/spm.conf"))
    }
}

pub fn sandbox_dir(name: &str) -> PathBuf {
    sandboxes_dir().join(name)
}

pub fn sam_archive(name: &str) -> PathBuf {
    archives_dir().join(format!("{name}.sam"))
}

pub fn backup_path(hash: &str) -> PathBuf {
    archives_dir().join(format!("{hash}.bak"))
}

pub fn trusted_keys_dir() -> PathBuf {
    maybe_root(PathBuf::from("/etc/spm/trusted-keys"))
}

pub fn scripts_dir() -> PathBuf {
    db_base().join("scripts")
}

pub fn triggers_dir() -> PathBuf {
    db_base().join("triggers")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_db_path() {
        let path = metadata_db();
        assert_eq!(path, PathBuf::from("/var/lib/spm/metadata.db"));
    }

    #[test]
    fn test_default_archives_dir() {
        let path = archives_dir();
        assert_eq!(path, PathBuf::from("/var/cache/spm/archives"));
    }

    #[test]
    fn test_default_packages_dir() {
        let path = packages_dir();
        assert_eq!(path, PathBuf::from("/var/lib/spm/packages"));
    }

    #[test]
    fn test_default_sandboxes_dir() {
        let path = sandboxes_dir();
        assert_eq!(path, PathBuf::from("/var/lib/spm/sandboxes"));
    }

    #[test]
    fn test_default_repos_config_dir() {
        let path = repos_config_dir();
        assert_eq!(path, PathBuf::from("/etc/spm/repos.d"));
    }

    #[test]
    fn test_default_config_file() {
        let path = config_file();
        assert_eq!(path, PathBuf::from("/etc/spm/spm.conf"));
    }

    #[test]
    fn test_user_config_file_respects_xdg() {
        let prev = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", "/custom/xdg");
        let path = user_config_file();
        assert_eq!(path, PathBuf::from("/custom/xdg/spm/spm.conf"));
        match prev {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
    }

    #[test]
    fn test_user_config_file_falls_back_to_home() {
        let prev_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        let prev_home = std::env::var("HOME").ok();
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::set_var("HOME", "/home/testuser");
        let path = user_config_file();
        assert_eq!(path, PathBuf::from("/home/testuser/.config/spm/spm.conf"));
        match prev_xdg {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn test_sandbox_dir() {
        let path = sandbox_dir("nginx");
        assert_eq!(path, PathBuf::from("/var/lib/spm/sandboxes/nginx"));
    }

    #[test]
    fn test_sam_archive() {
        let path = sam_archive("nginx");
        assert_eq!(path, PathBuf::from("/var/cache/spm/archives/nginx.sam"));
    }

    #[test]
    fn test_backup_path() {
        let path = backup_path("abc123");
        assert_eq!(path, PathBuf::from("/var/cache/spm/archives/abc123.bak"));
    }
}
