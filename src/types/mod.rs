mod package;
mod config;
mod version;
mod db;

pub use package::*;
pub use config::*;
pub use version::*;
pub use db::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spm_config_default() {
        let cfg = SpmConfig::default();
        assert!(cfg.db_path.is_none());
        assert!(cfg.cache_path.is_none());
        assert!(cfg.sandbox_path.is_none());
        assert!(cfg.log_level.is_none());
        assert!(cfg.auto_snapshot.is_none());
    }

    #[test]
    fn test_spm_config_serialize_roundtrip() {
        let cfg = SpmConfig {
            db_path: Some("/custom/db".into()),
            cache_path: Some("/custom/cache".into()),
            sandbox_path: Some("/custom/sandbox".into()),
            log_level: Some("debug".into()),
            auto_snapshot: Some(true),
            prefer_newest: None,
            auto_update_interval: None,
            preferred_source: None,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let deserialized: SpmConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.db_path, Some("/custom/db".into()));
        assert_eq!(deserialized.cache_path, Some("/custom/cache".into()));
        assert_eq!(deserialized.auto_snapshot, Some(true));
    }

    #[test]
    fn test_package_default() {
        let pkg = Package::default();
        assert_eq!(pkg.name, "");
        assert_eq!(pkg.version, "");
        assert!(pkg.dependencies.is_empty());
        assert_eq!(pkg.format, PackageFormat::Deb);
    }

    #[test]
    fn test_manifest_serialize_roundtrip() {
        let m = Manifest {
            name: "test-pkg".into(),
            version: "1.0.0".into(),
            architecture: "amd64".into(),
            maintainer: "test@example.com".into(),
            description: "A test package".into(),
            dependencies: vec![Dependency {
                name: "libc6".into(),
                version: ">=2.31".into(),
                source: DependencySource::System,
                format: Some(PackageFormat::Deb),
            }],
            conflicts: vec!["old-pkg".into()],
            provides: vec!["virtual-pkg".into()],
            recommends: vec!["ca-certificates".into()],
            install_size: 4096,
            format_version: 1,
            source: None,
            ai_metadata: None,
            signature: None,
            systemd_units: vec![],
            sysusers: vec![],
            tmpfiles: vec![],
            triggers: vec![],
            obsoletes: vec![],
            conffiles: vec![],
        };

        let json = serde_json::to_string(&m).unwrap();
        let deserialized: Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "test-pkg");
        assert_eq!(deserialized.dependencies[0].name, "libc6");
        assert_eq!(deserialized.format_version, 1);
    }

    #[test]
    fn test_transaction_serialize() {
        let tx = Transaction {
            id: Some(42),
            action: TransactionAction::Install,
            timestamp: "2026-05-30T12:00:00Z".into(),
            user: "root".into(),
            status: TransactionStatus::Completed,
            packages: vec!["nginx".into(), "libpcre3".into()],
            snapshot_id: None,
        };
        let json = serde_json::to_string(&tx).unwrap();
        let deserialized: Transaction = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, Some(42));
        assert!(matches!(deserialized.action, TransactionAction::Install));
        assert_eq!(deserialized.packages.len(), 2);
    }

    #[test]
    fn test_file_record_serialize() {
        let f = FileRecord {
            id: Some(1),
            transaction_id: 42,
            package: "nginx".into(),
            format: PackageFormat::Deb,
            filepath: "/usr/sbin/nginx".into(),
            hash: "abc123".into(),
            action: FileAction::Created,
        };
        let json = serde_json::to_string(&f).unwrap();
        let deserialized: FileRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.filepath, "/usr/sbin/nginx");
        assert!(matches!(deserialized.action, FileAction::Created));
    }

    #[test]
    fn test_installed_package() {
        let pkg = InstalledPackage {
            name: "nginx".into(),
            version: "1.27.0".into(),
            format: PackageFormat::Deb,
            install_type: InstallType::Native,
            manifest: None,
            install_date: "2026-05-30T12:00:00Z".into(),
            source_repo: Some("ubuntu noble".into()),
            store_hash: None,
            origin: InstallOrigin::Spm,
        };
        let json = serde_json::to_string(&pkg).unwrap();
        let deserialized: InstalledPackage = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "nginx");
        assert!(matches!(deserialized.install_type, InstallType::Native));
    }

    #[test]
    fn test_repo_config_serialize() {
        let rc = RepoConfig {
            source: RepoSource::Deb,
            distro: Some("ubuntu".into()),
            codename: Some("noble".into()),
            components: Some(vec!["main".into(), "universe".into()]),
            mirrors: Some(vec!["http://archive.ubuntu.com/ubuntu".into()]),
            release: None,
            repos: None,
            url: None,
            priority: None,
            signing_key: None,
        };
        let json = serde_json::to_string(&rc).unwrap();
        let deserialized: RepoConfig = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized.source, RepoSource::Deb));
        assert_eq!(deserialized.components.unwrap().len(), 2);
    }

    #[test]
    fn test_dependency_source_display() {
        let json = serde_json::to_string(&DependencySource::System).unwrap();
        assert_eq!(json, "\"System\"");
        let json = serde_json::to_string(&DependencySource::Spm).unwrap();
        assert_eq!(json, "\"Spm\"");
    }

    #[test]
    fn test_sandbox_level_serde() {
        assert_eq!(serde_json::to_string(&SandboxLevel::None).unwrap(), "\"none\"");
        assert_eq!(serde_json::to_string(&SandboxLevel::Full).unwrap(), "\"full\"");
    }

    #[test]
    fn test_version_parse() {
        let v = Version::parse("2:1.2.3-1");
        assert_eq!(v.epoch, 2);
        assert_eq!(v.version, "1.2.3");
        assert_eq!(v.release, "1");

        let v = Version::parse("1.0-1");
        assert_eq!(v.epoch, 0);
        assert_eq!(v.version, "1.0");
        assert_eq!(v.release, "1");

        let v = Version::parse("1.0");
        assert_eq!(v.release, "");
    }

    #[test]
    fn test_version_compare() {
        assert!(Version::compare("1.0-1", "1.0-2").is_lt());
        assert!(Version::compare("2.0-1", "1.0-1").is_gt());
        assert!(Version::compare("2:1.0-1", "1:2.0-1").is_gt());
        assert!(Version::compare("1:1.0-1", "1.0-1").is_gt());
        assert!(Version::compare("1.0-1", "1.0-1").is_eq());
        assert!(Version::compare("1.2.3-1", "1.2.10-1").is_lt());
        assert!(Version::compare("1.10-1", "1.2-1").is_gt());
        assert!(Version::compare("1.0a-1", "1.0b-1").is_lt());
        assert!(Version::compare("1.0~rc1-1", "1.0-1").is_lt());
        assert!(Version::compare("1.0~rc2-1", "1.0~rc1-1").is_gt());
        assert!(Version::compare("1.0-1.fc40", "1.0-1.fc39").is_gt());
    }

    #[test]
    fn test_rpmvercmp_basic() {
        assert_eq!(rpmvercmp("1", "2"), std::cmp::Ordering::Less);
        assert_eq!(rpmvercmp("2", "1"), std::cmp::Ordering::Greater);
        assert_eq!(rpmvercmp("1", "1"), std::cmp::Ordering::Equal);
        assert_eq!(rpmvercmp("1.2", "1.10"), std::cmp::Ordering::Less);
        assert_eq!(rpmvercmp("1.10", "1.2"), std::cmp::Ordering::Greater);
        assert_eq!(rpmvercmp("1a", "1b"), std::cmp::Ordering::Less);
        assert_eq!(rpmvercmp("1b", "1a"), std::cmp::Ordering::Greater);
        assert_eq!(rpmvercmp("1a", "1a"), std::cmp::Ordering::Equal);
        assert_eq!(rpmvercmp("1~rc1", "1"), std::cmp::Ordering::Less);
        assert_eq!(rpmvercmp("1", "1~rc1"), std::cmp::Ordering::Greater);
        assert_eq!(rpmvercmp("1~rc1", "1~rc2"), std::cmp::Ordering::Less);
    }
}
