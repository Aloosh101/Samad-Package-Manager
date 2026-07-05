pub mod namespaces;
pub mod seccomp;
pub mod landlock;
pub mod cgroups;
pub mod launcher;

use std::collections::HashMap;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use crate::error::{SpmError, SpmResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SandboxLevel {
    None,
    Standard,
    Strict,
    Full,
}

#[derive(Debug, Clone)]
pub struct MountSpec {
    pub source: PathBuf,
    pub target: PathBuf,
    pub fstype: Option<String>,
    pub readonly: bool,
}

#[derive(Debug, Clone)]
pub struct SandboxBuilder {
    level: SandboxLevel,
    mounts: Vec<MountSpec>,
    env: HashMap<String, String>,
}

impl SandboxBuilder {
    pub fn new(level: SandboxLevel) -> Self {
        Self {
            level,
            mounts: Vec::new(),
            env: HashMap::new(),
        }
    }

    pub fn add_mount(mut self, spec: MountSpec) -> Self {
        self.mounts.push(spec);
        self
    }

    pub fn set_env(mut self, key: String, value: String) -> Self {
        self.env.insert(key, value);
        self
    }

    pub fn spawn(self, cmd: &str, args: &[&str]) -> SpmResult<launcher::SandboxProcess> {
        let mut child = Command::new(cmd);
        child.args(args);

        for (k, v) in &self.env {
            child.env(k, v);
        }

        let need_mount = self.level >= SandboxLevel::Standard;
        let need_user = self.level >= SandboxLevel::Strict;
        let need_net = self.level >= SandboxLevel::Strict;
        let need_cgroup = self.level == SandboxLevel::Full;

        let mounts = self.mounts.clone();

        let seccomp_profile = if self.level >= SandboxLevel::Strict {
            Some(seccomp::SeccompProfile::default_allowlist())
        } else {
            None
        };

        let landlock_rules = if self.level >= SandboxLevel::Strict {
            Some(landlock::LandlockRules::default())
        } else {
            None
        };

        let cgroup_limits = if need_cgroup {
            Some(cgroups::CgroupLimits::default())
        } else {
            None
        };

        // Create cgroup before spawning so we can add the PID after
        if let Some(ref limits) = cgroup_limits {
            limits.create()?;
        }

        #[cfg(target_os = "linux")]
        unsafe {
            child.pre_exec({
                let mounts = mounts.clone();
                let seccomp = seccomp_profile.clone();
                let landlock = landlock_rules.clone();

                move || {
                    namespaces::unshare_namespaces(
                        need_mount,
                        need_user,
                        need_net,
                    )?;

                    if need_mount {
                        namespaces::setup_mount_namespace(&mounts)?;
                    }

                    if need_user {
                        namespaces::setup_user_namespace()?;
                    }

                    namespaces::setup_uts_namespace()?;

                    namespaces::set_no_new_privs()?;
                    namespaces::drop_all_capabilities()?;
                    namespaces::set_rlimits()?;

                    if let Some(ref profile) = seccomp {
                        profile.apply()?;
                    }

                    if let Some(ref rules) = landlock {
                        rules.apply()?;
                    }

                    Ok(())
                }
            });
        }

        #[cfg(not(target_os = "linux"))]
        unsafe {
            child.pre_exec(move || Ok(()));
        }

        let mut child_handle = child.spawn().map_err(|e| {
            SpmError::command_failed(format!("Failed to spawn sandboxed process: {e}"))
        })?;

        if let Some(ref limits) = cgroup_limits {
            let pid = child_handle.id();
            if let Err(e) = limits.add_pid(pid) {
                let _ = child_handle.kill();
                return Err(SpmError::sandbox(format!("Failed to add PID to cgroup: {e}")));
            }
        }

        Ok(launcher::SandboxProcess {
            child: child_handle,
        })
    }
}
