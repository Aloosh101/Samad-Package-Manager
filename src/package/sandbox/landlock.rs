#[cfg(target_os = "linux")]
mod imp {
    use std::ffi::CString;
    use std::path::Path;
    use std::path::PathBuf;

    const LANDLOCK_CREATE_RULESET_SYSCALL: i64 = 444;
    const LANDLOCK_ADD_RULE_SYSCALL: i64 = 445;
    const LANDLOCK_RESTRICT_SELF_SYSCALL: i64 = 446;

    const LANDLOCK_RULE_PATH_BENEATH: u32 = 1;

    const LANDLOCK_ACCESS_FS_EXECUTE: u64 = 1 << 0;
    const LANDLOCK_ACCESS_FS_WRITE_FILE: u64 = 1 << 1;
    const LANDLOCK_ACCESS_FS_READ_FILE: u64 = 1 << 2;
    const LANDLOCK_ACCESS_FS_READ_DIR: u64 = 1 << 3;

    #[repr(C)]
    struct landlock_ruleset_attr {
        handled_access_fs: u64,
    }

    #[repr(C, packed)]
    struct landlock_path_beneath_attr {
        allowed_access: u64,
        parent_fd: i32,
    }

    #[derive(Clone, Copy, Debug)]
    pub struct AccessFs(pub u64);

    impl AccessFs {
        pub const READ: AccessFs = AccessFs(LANDLOCK_ACCESS_FS_READ_FILE | LANDLOCK_ACCESS_FS_READ_DIR);
        pub const WRITE: AccessFs = AccessFs(LANDLOCK_ACCESS_FS_WRITE_FILE);
        pub const EXECUTE: AccessFs = AccessFs(LANDLOCK_ACCESS_FS_EXECUTE);
    }

    #[derive(Clone, Debug)]
    pub struct LandlockRule {
        pub path: PathBuf,
        pub access: AccessFs,
    }

    #[derive(Clone, Default)]
    pub struct LandlockRules {
        pub rules: Vec<LandlockRule>,
    }

    impl LandlockRules {
        pub fn default() -> Self {
            Self { rules: Vec::new() }
        }

        pub fn allow_read(mut self, path: &Path) -> Self {
            self.rules.push(LandlockRule {
                path: path.to_path_buf(),
                access: AccessFs::READ,
            });
            self
        }

        pub fn allow_write(mut self, path: &Path) -> Self {
            self.rules.push(LandlockRule {
                path: path.to_path_buf(),
                access: AccessFs::WRITE,
            });
            self
        }

        pub fn allow_exec(mut self, path: &Path) -> Self {
            self.rules.push(LandlockRule {
                path: path.to_path_buf(),
                access: AccessFs::EXECUTE,
            });
            self
        }

        pub fn apply(&self) -> Result<(), std::io::Error> {
            if self.rules.is_empty() {
                return Ok(());
            }

            let attr = landlock_ruleset_attr {
                handled_access_fs: LANDLOCK_ACCESS_FS_EXECUTE
                    | LANDLOCK_ACCESS_FS_WRITE_FILE
                    | LANDLOCK_ACCESS_FS_READ_FILE
                    | LANDLOCK_ACCESS_FS_READ_DIR,
            };

            let ruleset_fd = unsafe {
                libc::syscall(
                    LANDLOCK_CREATE_RULESET_SYSCALL,
                    &attr as *const landlock_ruleset_attr,
                    std::mem::size_of::<landlock_ruleset_attr>(),
                    0u32,
                ) as libc::c_int
            };

            if ruleset_fd < 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::ENOSYS)
                    || err.raw_os_error() == Some(libc::EOPNOTSUPP)
                {
                    return Ok(());
                }
                return Err(err);
            }

            for rule in &self.rules {
                let path_c = CString::new(rule.path.as_os_str().as_encoded_bytes())
                    .map_err(|_| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "invalid path for landlock rule",
                        )
                    })?;

                let fd = unsafe {
                    libc::open(
                        path_c.as_ptr(),
                        libc::O_RDONLY | libc::O_CLOEXEC,
                    )
                };
                if fd < 0 {
                    unsafe { libc::close(ruleset_fd) };
                    return Err(std::io::Error::last_os_error());
                }

                let path_beneath = landlock_path_beneath_attr {
                    allowed_access: rule.access.0,
                    parent_fd: fd,
                };

                let ret = unsafe {
                    libc::syscall(
                        LANDLOCK_ADD_RULE_SYSCALL,
                        ruleset_fd as libc::c_int,
                        LANDLOCK_RULE_PATH_BENEATH,
                        &path_beneath as *const landlock_path_beneath_attr,
                        0u32,
                    )
                };
                unsafe { libc::close(fd) };

                if ret != 0 {
                    unsafe { libc::close(ruleset_fd) };
                    return Err(std::io::Error::last_os_error());
                }
            }

            let ret = unsafe {
                libc::syscall(
                    LANDLOCK_RESTRICT_SELF_SYSCALL,
                    ruleset_fd as libc::c_int,
                    0u32,
                )
            };
            unsafe { libc::close(ruleset_fd) };

            if ret != 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::ENOSYS)
                    || err.raw_os_error() == Some(libc::EOPNOTSUPP)
                {
                    return Ok(());
                }
                return Err(err);
            }

            Ok(())
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use std::path::Path;
    use std::path::PathBuf;

    #[derive(Clone, Copy, Debug)]
    pub struct AccessFs(pub u64);

    impl AccessFs {
        pub const READ: AccessFs = AccessFs(1);
        pub const WRITE: AccessFs = AccessFs(2);
        pub const EXECUTE: AccessFs = AccessFs(4);
    }

    #[derive(Clone, Debug)]
    pub struct LandlockRule {
        pub path: PathBuf,
        pub access: AccessFs,
    }

    #[derive(Clone, Default)]
    pub struct LandlockRules {
        pub rules: Vec<LandlockRule>,
    }

    impl LandlockRules {
        pub fn default() -> Self {
            Self { rules: Vec::new() }
        }

        pub fn allow_read(mut self, path: &Path) -> Self {
            self.rules.push(LandlockRule {
                path: path.to_path_buf(),
                access: AccessFs::READ,
            });
            self
        }

        pub fn allow_write(mut self, path: &Path) -> Self {
            self.rules.push(LandlockRule {
                path: path.to_path_buf(),
                access: AccessFs::WRITE,
            });
            self
        }

        pub fn allow_exec(mut self, path: &Path) -> Self {
            self.rules.push(LandlockRule {
                path: path.to_path_buf(),
                access: AccessFs::EXECUTE,
            });
            self
        }

        pub fn apply(&self) -> Result<(), std::io::Error> {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "landlock not supported on this platform",
            ))
        }
    }
}

pub use imp::{AccessFs, LandlockRule, LandlockRules};
