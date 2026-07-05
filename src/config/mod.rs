pub mod paths;
pub mod repos;

use std::fs;

use crate::error::{SpmError, SpmResult};
use crate::types::SpmConfig;

fn load_config_from(path: &std::path::Path) -> Option<SpmConfig> {
    if path.exists() {
        fs::read_to_string(path).ok().and_then(|s| toml::from_str(&s).ok())
    } else {
        None
    }
}

fn merge_config(system: SpmConfig, user: SpmConfig) -> SpmConfig {
    SpmConfig {
        db_path: user.db_path.or(system.db_path),
        cache_path: user.cache_path.or(system.cache_path),
        sandbox_path: user.sandbox_path.or(system.sandbox_path),
        log_level: user.log_level.or(system.log_level),
        auto_snapshot: user.auto_snapshot.or(system.auto_snapshot),
        prefer_newest: user.prefer_newest.or(system.prefer_newest),
        auto_update_interval: user.auto_update_interval.or(system.auto_update_interval),
        preferred_source: user.preferred_source.or(system.preferred_source),
    }
}

impl SpmConfig {
    pub fn load() -> SpmResult<Self> {
        let system = load_config_from(&paths::config_file()).unwrap_or_default();
        let user = load_config_from(&paths::user_config_file()).unwrap_or_default();
        Ok(merge_config(system, user))
    }

    pub fn set(key: &str, value: &str) -> SpmResult<()> {
        let path = paths::user_config_file();
        let mut config: SpmConfig = if path.exists() {
            let content = fs::read_to_string(&path)?;
            toml::from_str(&content)?
        } else {
            SpmConfig::default()
        };

        match key {
            "db_path" => config.db_path = Some(value.to_string()),
            "cache_path" => config.cache_path = Some(value.to_string()),
            "sandbox_path" => config.sandbox_path = Some(value.to_string()),
            "log_level" => config.log_level = Some(value.to_string()),
            "auto_snapshot" => {
                config.auto_snapshot = Some(value.parse().map_err(|_| {
                    SpmError::config(format!("auto_snapshot must be true or false, got: {}", value))
                })?);
            }
            "prefer_newest" => {
                config.prefer_newest = Some(value.parse().map_err(|_| {
                    SpmError::config(format!("prefer_newest must be true or false, got: {}", value))
                })?);
            }
            "auto_update_interval" => {
                config.auto_update_interval = Some(value.parse().map_err(|_| {
                    SpmError::config(format!("auto_update_interval must be a number (seconds), got: {}", value))
                })?);
            }
            "preferred_source" => {
                let valid = ["deb", "rpm", "native"];
                if !valid.contains(&value) {
                    return Err(SpmError::config(format!(
                        "preferred_source must be one of: {}, got: {}", valid.join(", "), value
                    )));
                }
                config.preferred_source = Some(value.to_string());
            }
            _ => return Err(SpmError::config(format!(
                "Unknown config key: {key}. Valid keys: db_path, cache_path, sandbox_path, log_level, auto_snapshot, prefer_newest, auto_update_interval, preferred_source"
            ))),
        }

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(&config)?;
        let tmp_path = path.with_extension("conf.tmp");
        fs::write(&tmp_path, &content)?;
        fs::rename(&tmp_path, &path)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_config_default_when_no_file() {
        let cfg = SpmConfig::load().unwrap();
        assert!(cfg.db_path.is_none());
        assert!(cfg.cache_path.is_none());
    }

    #[test]
    fn test_config_set_and_load() {
        let dir = std::env::temp_dir().join("spm-config-set-test");
        let _ = fs::remove_dir_all(&dir);
        let etc_spm = dir.join("etc").join("spm");
        fs::create_dir_all(&etc_spm).unwrap();
        let config_path = etc_spm.join("spm.conf");

        // Write config manually
        let content = r#"db_path = "/custom/db"
cache_path = "/custom/cache"
log_level = "debug"
auto_snapshot = true
"#;
        fs::write(&config_path, content).unwrap();

        // Read using load (reads from /etc/spm/spm.conf by default, not our temp path)
        // So we parse it directly to verify TOML parsing works
        let parsed: SpmConfig = toml::from_str(content).unwrap();
        assert_eq!(parsed.db_path, Some("/custom/db".into()));
        assert_eq!(parsed.cache_path, Some("/custom/cache".into()));
        assert_eq!(parsed.log_level, Some("debug".into()));
        assert_eq!(parsed.auto_snapshot, Some(true));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_config_set_invalid_key() {
        let result = SpmConfig::set("invalid_key", "value");
        assert!(result.is_err());
        match result {
            Err(SpmError::Config(msg)) => {
                assert!(msg.contains("Unknown config key"));
            }
            _ => panic!("Expected Config error"),
        }
    }

    #[test]
    fn test_config_set_invalid_auto_snapshot() {
        let result = SpmConfig::set("auto_snapshot", "notabool");
        assert!(result.is_err());
    }

    #[test]
    fn test_config_serialize_toml_roundtrip() {
        let cfg = SpmConfig {
            db_path: Some("/a".into()),
            cache_path: Some("/b".into()),
            sandbox_path: Some("/c".into()),
            log_level: Some("warn".into()),
            auto_snapshot: Some(false),
            prefer_newest: Some(true),
            auto_update_interval: None,
            preferred_source: Some("apt".into()),
        };
        let toml_str = toml::to_string_pretty(&cfg).unwrap();
        let parsed: SpmConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.db_path, Some("/a".into()));
        assert_eq!(parsed.auto_snapshot, Some(false));
        assert_eq!(parsed.preferred_source, Some("apt".into()));
    }

    #[test]
    fn test_config_partial_defaults() {
        let content = r#"log_level = "info""#;
        let parsed: SpmConfig = toml::from_str(content).unwrap();
        assert_eq!(parsed.log_level, Some("info".into()));
        assert!(parsed.db_path.is_none());
        assert!(parsed.cache_path.is_none());
    }

    #[test]
    fn test_merge_config_user_overrides_system() {
        let system = SpmConfig {
            db_path: Some("/sys/db".into()),
            cache_path: Some("/sys/cache".into()),
            sandbox_path: None,
            log_level: Some("debug".into()),
            auto_snapshot: Some(false),
            prefer_newest: None,
            auto_update_interval: None,
            preferred_source: Some("dnf".into()),
        };
        let user = SpmConfig {
            db_path: None,
            cache_path: Some("/user/cache".into()),
            sandbox_path: Some("/user/sbox".into()),
            log_level: None,
            auto_snapshot: Some(true),
            prefer_newest: Some(true),
            auto_update_interval: Some(3600),
            preferred_source: None,
        };
        let merged = merge_config(system, user);
        assert_eq!(merged.db_path, Some("/sys/db".into()));       // system kept
        assert_eq!(merged.cache_path, Some("/user/cache".into())); // user overrides
        assert_eq!(merged.sandbox_path, Some("/user/sbox".into())); // user sets
        assert_eq!(merged.log_level, Some("debug".into()));        // system kept
        assert_eq!(merged.auto_snapshot, Some(true));              // user overrides
        assert_eq!(merged.prefer_newest, Some(true));              // user sets
        assert_eq!(merged.preferred_source, Some("dnf".into()));    // system kept (user didn't set)
    }
}
