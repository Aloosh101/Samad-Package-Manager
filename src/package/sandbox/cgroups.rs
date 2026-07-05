#[cfg(target_os = "linux")]
mod imp {
    use std::fs;
    use std::path::PathBuf;

    const CGROUP_BASE: &str = "/sys/fs/cgroup";

    #[derive(Debug, Clone)]
    pub struct CgroupLimits {
        pub memory_max: u64,
        pub cpu_quota: u32,
        pub cpu_period: u32,
        pub pids_max: u32,
        #[allow(dead_code)]
        path: PathBuf,
    }

    impl Default for CgroupLimits {
        fn default() -> Self {
            Self {
                memory_max: 512 * 1024 * 1024,
                cpu_quota: 50000,
                cpu_period: 100000,
                pids_max: 64,
                path: PathBuf::new(),
            }
        }
    }

    impl CgroupLimits {
        pub fn create(&self) -> crate::error::SpmResult<()> {
            use crate::error::SpmError;

            let pid = std::process::id();
            let path = PathBuf::from(format!("{}/spm-{}", CGROUP_BASE, pid));

            fs::create_dir_all(&path).map_err(|e| {
                SpmError::sandbox(format!("Failed to create cgroup directory: {e}"))
            })?;

            fs::write(path.join("memory.max"), format!("{}\n", self.memory_max)).map_err(|e| {
                SpmError::sandbox(format!("Failed to set memory.max: {e}"))
            })?;

            fs::write(
                path.join("cpu.max"),
                format!("{} {}\n", self.cpu_quota, self.cpu_period),
            )
            .map_err(|e| {
                SpmError::sandbox(format!("Failed to set cpu.max: {e}"))
            })?;

            fs::write(path.join("pids.max"), format!("{}\n", self.pids_max)).map_err(|e| {
                SpmError::sandbox(format!("Failed to set pids.max: {e}"))
            })?;

            Ok(())
        }

        pub fn add_pid(&self, pid: u32) -> crate::error::SpmResult<()> {
            use crate::error::SpmError;

            let cgroup_path = PathBuf::from(format!("{}/spm-{}", CGROUP_BASE, std::process::id()));
            let procs_path = cgroup_path.join("cgroup.procs");

            fs::write(&procs_path, format!("{}\n", pid)).map_err(|e| {
                SpmError::sandbox(format!("Failed to add PID {pid} to cgroup: {e}"))
            })
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    #[derive(Debug, Clone)]
    pub struct CgroupLimits;

    impl Default for CgroupLimits {
        fn default() -> Self {
            Self
        }
    }

    impl CgroupLimits {
        pub fn create(&self) -> crate::error::SpmResult<()> {
            Err(crate::error::SpmError::sandbox(
                "cgroups not supported on this platform",
            ))
        }

        pub fn add_pid(&self, _pid: u32) -> crate::error::SpmResult<()> {
            Err(crate::error::SpmError::sandbox(
                "cgroups not supported on this platform",
            ))
        }
    }
}

pub use imp::CgroupLimits;
