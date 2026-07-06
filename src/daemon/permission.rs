use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::error::{SpmError, SpmResult};

#[derive(Debug, Clone)]
pub struct PeerCreds {
    pub uid: u32,
    pub gid: u32,
    pub groups: Vec<u32>,
}

impl PeerCreds {
    pub fn new(uid: u32, gid: u32) -> Self {
        Self {
            uid,
            gid,
            groups: Vec::new(),
        }
    }

    pub fn with_groups(mut self, groups: Vec<u32>) -> Self {
        self.groups = groups;
        self
    }

    pub fn resolve(uid: u32, gid: u32) -> Self {
        let groups = resolve_supplementary_groups(uid, gid);
        Self { uid, gid, groups }
    }
}

#[derive(Debug, Clone)]
pub enum Permission {
    SystemFull,
    ReadOnly,
    UserOnly { uid: u32, home: String },
    Denied,
}

impl Permission {
    pub fn is_authorized(&self) -> bool {
        !matches!(self, Permission::Denied)
    }

    pub fn is_system(&self) -> bool {
        matches!(self, Permission::SystemFull | Permission::ReadOnly)
    }

    pub fn uid(&self) -> Option<u32> {
        match self {
            Permission::UserOnly { uid, .. } => Some(*uid),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum FileAccess {
    Allowed,
    ForeignTouch {
        owner_pkg: String,
        owner_creds: PeerCreds,
    },
    NotOwnedBySpm,
}

#[derive(Debug, Clone)]
pub struct PermissionPolicy {
    pub root_can_everything: bool,
    pub group_spm_can_system: bool,
    pub users_can_user_install: bool,
    pub sandbox_required_for: Vec<String>,
    pub blocked_packages: Vec<String>,
    pub max_concurrent_per_user: u32,
    pub rate_limit_per_user: u32,
}

impl Default for PermissionPolicy {
    fn default() -> Self {
        Self {
            root_can_everything: true,
            group_spm_can_system: true,
            users_can_user_install: true,
            sandbox_required_for: Vec::new(),
            blocked_packages: Vec::new(),
            max_concurrent_per_user: 3,
            rate_limit_per_user: 10,
        }
    }
}

impl PermissionPolicy {
    pub fn sandbox_required(&self, pkg: &str) -> bool {
        self.sandbox_required_for.iter().any(|s| s == pkg)
    }

    pub fn is_blocked(&self, pkg: &str) -> bool {
        self.blocked_packages.iter().any(|b| b == pkg)
    }
}

const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(1);
const RATE_LIMIT_MAX: u32 = 10;

pub struct PermissionEngine {
    policy: PermissionPolicy,
    rate_limits: Mutex<HashMap<u32, (Instant, u32)>>,
}

impl PermissionEngine {
    pub fn new(policy: PermissionPolicy) -> Self {
        Self {
            policy,
            rate_limits: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_defaults() -> Self {
        Self {
            policy: PermissionPolicy::default(),
            rate_limits: Mutex::new(HashMap::new()),
        }
    }

    pub fn policy(&self) -> &PermissionPolicy {
        &self.policy
    }

    /// Simplified file access check by user ID
    pub fn check_file_access_by_uid(&self, path: &Path, uid: u32) -> bool {
        if self.policy.root_can_everything && uid == 0 {
            return true;
        }
        let home = resolve_user_home(uid).unwrap_or_else(|| format!("/home/{uid}"));
        let path_str = path.to_string_lossy();
        if path_str.starts_with(&home) {
            return true;
        }
        is_spm_managed_path(&path_str)
    }

    /// Rate limit check: max 10 requests/second per user
    pub fn check_rate_limit(&self, uid: u32) -> bool {
        let mut limits = match self.rate_limits.lock() {
            Ok(l) => l,
            Err(_) => return false,
        };
        let now = Instant::now();
        let entry = limits.entry(uid).or_insert((now, 0));
        if now.duration_since(entry.0) > RATE_LIMIT_WINDOW {
            *entry = (now, 1);
            true
        } else if entry.1 >= RATE_LIMIT_MAX {
            false
        } else {
            entry.1 += 1;
            true
        }
    }

    pub fn check(
        &self,
        action: &str,
        package: Option<&str>,
        creds: &PeerCreds,
    ) -> SpmResult<Permission> {
        // Root can do everything
        if self.policy.root_can_everything && creds.uid == 0 {
            return Ok(Permission::SystemFull);
        }

        // Check blocked packages
        if let Some(pkg) = package {
            if self.policy.is_blocked(pkg) {
                return Err(SpmError::permission_denied(format!(
                    "Package '{}' is blocked by policy",
                    pkg
                )));
            }
        }

        // Check if user is in 'spm' group → system-level access
        let spm_gid = resolve_spm_gid();
        let is_spm_group = self.policy.group_spm_can_system
            && spm_gid != 0
            && (creds.gid == spm_gid || creds.groups.contains(&spm_gid));

        if is_spm_group {
            match action {
                "list" | "info" | "search" | "status" => Ok(Permission::ReadOnly),
                _ => Ok(Permission::SystemFull),
            }
        } else {
            // Check sandbox requirements before granting user install
            if let Some(pkg) = package {
                if self.policy.sandbox_required(pkg) {
                    return Err(SpmError::permission_denied(format!(
                        "Package '{}' requires sandbox-mode installation",
                        pkg
                    )));
                }
            }

            // Ordinary users
            if self.policy.users_can_user_install {
                match action {
                    "install" | "remove" | "list" | "info" | "search" | "status" => {
                        let home = resolve_user_home(creds.uid)
                            .unwrap_or_else(|| format!("/home/{}", creds.uid));
                        return Ok(Permission::UserOnly {
                            uid: creds.uid,
                            home,
                        });
                    }
                    _ => {}
                }
            }

            Ok(Permission::Denied)
        }
    }

    pub fn check_file_access(&self, path: &str, creds: &PeerCreds) -> FileAccess {
        if self.policy.root_can_everything && creds.uid == 0 {
            return FileAccess::Allowed;
        }

        let home = resolve_user_home(creds.uid).unwrap_or_else(|| format!("/home/{}", creds.uid));

        if path.starts_with(&home) {
            return FileAccess::Allowed;
        }

        if !is_spm_managed_path(path) {
            return FileAccess::NotOwnedBySpm;
        }

        FileAccess::Allowed
    }
}

fn resolve_spm_gid() -> u32 {
    use nix::unistd::Group;
    Group::from_name("spm")
        .ok()
        .flatten()
        .map(|g| g.gid.as_raw())
        .unwrap_or(0)
}

fn resolve_user_home(uid: u32) -> Option<String> {
    if uid == 0 {
        return Some("/root".to_string());
    }
    use nix::unistd::{Uid, User};
    User::from_uid(Uid::from_raw(uid))
        .ok()
        .flatten()
        .map(|u| u.dir.to_string_lossy().into_owned())
}

fn resolve_supplementary_groups(uid: u32, _gid: u32) -> Vec<u32> {
    use nix::unistd::User;
    let username = match crate::util::user::resolve_user_name(uid) {
        Some(n) => n,
        None => return Vec::new(),
    };
    // Use nix::unistd::User::from_name for a thread-safe lookup
    let user = match User::from_name(&username) {
        Ok(Some(u)) => u,
        _ => return Vec::new(),
    };
    // Return primary group as the minimal set; supplementary groups
    // are resolved via the daemon's is_authorized check using getgrouplist.
    vec![user.gid.as_raw()]
}

fn is_spm_managed_path(path: &str) -> bool {
    let spm_paths = ["/usr/lib/spm", "/var/lib/spm", "/etc/spm"];
    spm_paths.iter().any(|p| path.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root_creds() -> PeerCreds {
        PeerCreds::new(0, 0)
    }

    fn user_creds(uid: u32) -> PeerCreds {
        PeerCreds::new(uid, 1000)
    }

    #[test]
    fn test_root_gets_system_full() {
        let engine = PermissionEngine::with_defaults();
        let perm = engine
            .check("install", Some("nginx"), &root_creds())
            .unwrap();
        assert!(matches!(perm, Permission::SystemFull));
    }

    #[test]
    fn test_root_with_policy_disabled() {
        let policy = PermissionPolicy {
            root_can_everything: false,
            users_can_user_install: false,
            ..Default::default()
        };
        let engine = PermissionEngine::new(policy);
        let perm = engine
            .check("install", Some("nginx"), &root_creds())
            .unwrap();
        assert!(matches!(perm, Permission::Denied));
    }

    #[test]
    fn test_regular_user_install() {
        let engine = PermissionEngine::with_defaults();
        let creds = user_creds(1001);
        let perm = engine.check("install", Some("figlet"), &creds).unwrap();
        assert!(matches!(perm, Permission::UserOnly { uid: 1001, .. }));
    }

    #[test]
    fn test_regular_user_blocked_action() {
        let engine = PermissionEngine::with_defaults();
        let creds = user_creds(1001);
        let perm = engine.check("dist-upgrade", None, &creds).unwrap();
        assert!(matches!(perm, Permission::Denied));
    }

    #[test]
    fn test_blocked_package() {
        let policy = PermissionPolicy {
            blocked_packages: vec!["malware".to_string()],
            ..Default::default()
        };
        let engine = PermissionEngine::new(policy);
        let result = engine.check("install", Some("malware"), &user_creds(1001));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("blocked"));
    }

    #[test]
    fn test_sandbox_required() {
        let policy = PermissionPolicy {
            sandbox_required_for: vec!["untrusted".to_string()],
            ..Default::default()
        };
        let engine = PermissionEngine::new(policy);
        let creds = user_creds(1001);
        let result = engine.check("install", Some("untrusted"), &creds);
        assert!(result.is_err());
    }

    #[test]
    fn test_read_only_actions_for_ordinary_user() {
        let engine = PermissionEngine::with_defaults();
        let creds = user_creds(1001);
        for action in &["list", "info", "search", "status"] {
            let perm = engine.check(action, None, &creds).unwrap();
            assert!(
                matches!(perm, Permission::UserOnly { .. }),
                "{} should be allowed for regular users",
                action
            );
        }
    }

    #[test]
    fn test_file_access_root() {
        let engine = PermissionEngine::with_defaults();
        assert!(matches!(
            engine.check_file_access("/etc/shadow", &root_creds()),
            FileAccess::Allowed
        ));
    }

    #[test]
    fn test_file_access_user_home() {
        let engine = PermissionEngine::with_defaults();
        let creds = PeerCreds::new(1001, 1001);
        let result = engine.check_file_access("/home/1001/.local", &creds);
        assert!(matches!(result, FileAccess::Allowed));
    }

    #[test]
    fn test_file_access_not_owned() {
        let engine = PermissionEngine::with_defaults();
        let creds = PeerCreds::new(1001, 1001);
        let result = engine.check_file_access("/etc/passwd", &creds);
        assert!(matches!(result, FileAccess::NotOwnedBySpm));
    }

    #[test]
    fn test_permission_is_authorized() {
        assert!(Permission::SystemFull.is_authorized());
        assert!(Permission::ReadOnly.is_authorized());
        assert!(Permission::UserOnly {
            uid: 1000,
            home: "/home/user".into()
        }
        .is_authorized());
        assert!(!Permission::Denied.is_authorized());
    }

    #[test]
    fn test_permission_uid() {
        assert_eq!(Permission::SystemFull.uid(), None);
        assert_eq!(
            Permission::UserOnly {
                uid: 42,
                home: "/home/test".into()
            }
            .uid(),
            Some(42)
        );
        assert_eq!(Permission::Denied.uid(), None);
    }
}
