use std::ffi::CString;

use super::MountSpec;

#[cfg(target_os = "linux")]
pub fn unshare_namespaces(need_mount: bool, need_user: bool, need_net: bool) -> Result<(), std::io::Error> {
    use nix::sched::{unshare, CloneFlags};

    let mut flags = CloneFlags::empty();
    flags.set(CloneFlags::CLONE_NEWPID, true);
    flags.set(CloneFlags::CLONE_NEWUTS, true);
    if need_mount {
        flags.set(CloneFlags::CLONE_NEWNS, true);
    }
    if need_user {
        flags.set(CloneFlags::CLONE_NEWUSER, true);
    }
    if need_net {
        flags.set(CloneFlags::CLONE_NEWNET, true);
    }

    unshare(flags).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("unshare failed: {e}"),
        )
    })
}

#[cfg(not(target_os = "linux"))]
pub fn unshare_namespaces(_need_mount: bool, _need_user: bool, _need_net: bool) -> Result<(), std::io::Error> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "namespaces not supported on this platform",
    ))
}

#[cfg(target_os = "linux")]
pub fn setup_mount_namespace(mounts: &[MountSpec]) -> Result<(), std::io::Error> {
    let root = CString::new("/").map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid path")
    })?;

    let ret = unsafe {
        libc::mount(
            std::ptr::null(),
            root.as_ptr(),
            std::ptr::null(),
            (libc::MS_PRIVATE | libc::MS_REC) as libc::c_ulong,
            std::ptr::null(),
        )
    };
    if ret != 0 {
        return Err(std::io::Error::last_os_error());
    }

    for spec in mounts {
        apply_mount_spec(spec)?;
    }

    let ret = unsafe {
        libc::mount(
            std::ptr::null(),
            root.as_ptr(),
            std::ptr::null(),
            (libc::MS_REMOUNT | libc::MS_RDONLY | libc::MS_BIND) as libc::c_ulong,
            std::ptr::null(),
        )
    };
    if ret != 0 {
        return Err(std::io::Error::last_os_error());
    }

    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn setup_mount_namespace(_mounts: &[MountSpec]) -> Result<(), std::io::Error> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "mount namespace not supported on this platform",
    ))
}

#[cfg(target_os = "linux")]
fn apply_mount_spec(spec: &MountSpec) -> Result<(), std::io::Error> {
    let source = CString::new(spec.source.as_os_str().as_encoded_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid source path")
    })?;
    let target = CString::new(spec.target.as_os_str().as_encoded_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid target path")
    })?;

    let ret = unsafe {
        libc::mount(
            source.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            libc::MS_BIND as libc::c_ulong,
            std::ptr::null(),
        )
    };
    if ret != 0 {
        return Err(std::io::Error::last_os_error());
    }

    if spec.readonly {
        let ret = unsafe {
            libc::mount(
                std::ptr::null(),
                target.as_ptr(),
                std::ptr::null(),
                (libc::MS_REMOUNT | libc::MS_BIND | libc::MS_RDONLY) as libc::c_ulong,
                std::ptr::null(),
            )
        };
        if ret != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }

    Ok(())
}

#[cfg(target_os = "linux")]
pub fn setup_user_namespace() -> Result<(), std::io::Error> {
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };

    let uid_map = format!("{uid} {uid} 1\n");
    let gid_map = format!("{gid} {gid} 1\n");

    let uid_map_c = CString::new(uid_map).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid uid_map")
    })?;
    let gid_map_c = CString::new(gid_map).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid gid_map")
    })?;

    let uid_fd = unsafe {
        libc::open(
            b"/proc/self/uid_map\0".as_ptr() as *const libc::c_char,
            libc::O_WRONLY | libc::O_CLOEXEC,
        )
    };
    if uid_fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let ret = unsafe {
        libc::write(
            uid_fd,
            uid_map_c.as_ptr() as *const libc::c_void,
            uid_map_c.as_bytes().len(),
        )
    };
    unsafe { libc::close(uid_fd) };
    if ret < 0 {
        return Err(std::io::Error::last_os_error());
    }

    let gid_fd = unsafe {
        libc::open(
            b"/proc/self/setgroups\0".as_ptr() as *const libc::c_char,
            libc::O_WRONLY | libc::O_CLOEXEC,
        )
    };
    if gid_fd >= 0 {
        let deny = b"deny\n";
        unsafe {
            libc::write(
                gid_fd,
                deny.as_ptr() as *const libc::c_void,
                deny.len(),
            );
            libc::close(gid_fd);
        }
    }

    let gid_fd2 = unsafe {
        libc::open(
            b"/proc/self/gid_map\0".as_ptr() as *const libc::c_char,
            libc::O_WRONLY | libc::O_CLOEXEC,
        )
    };
    if gid_fd2 < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let ret = unsafe {
        libc::write(
            gid_fd2,
            gid_map_c.as_ptr() as *const libc::c_void,
            gid_map_c.as_bytes().len(),
        )
    };
    unsafe { libc::close(gid_fd2) };
    if ret < 0 {
        return Err(std::io::Error::last_os_error());
    }

    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn setup_user_namespace() -> Result<(), std::io::Error> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "user namespace not supported on this platform",
    ))
}

#[cfg(target_os = "linux")]
pub fn setup_uts_namespace() -> Result<(), std::io::Error> {
    let hostname = b"sandbox";
    let ret = unsafe {
        libc::sethostname(
            hostname.as_ptr() as *const libc::c_char,
            hostname.len(),
        )
    };
    if ret != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn setup_uts_namespace() -> Result<(), std::io::Error> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "UTS namespace not supported on this platform",
    ))
}

#[cfg(target_os = "linux")]
pub fn set_no_new_privs() -> Result<(), std::io::Error> {
    let ret = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
    if ret != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn set_no_new_privs() -> Result<(), std::io::Error> {
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn drop_all_capabilities() -> Result<(), std::io::Error> {
    for cap in 0..64u64 {
        let ret = unsafe {
            libc::prctl(libc::PR_CAPBSET_DROP, cap, 0, 0, 0)
        };
        if ret != 0 {
            // If the capability doesn't exist (invalid cap), that's fine
            if unsafe { *libc::__errno_location() } != libc::EINVAL {
                return Err(std::io::Error::last_os_error());
            }
        }
    }
    // Verify bounding set is empty
    for cap in 0..64u64 {
        let ret = unsafe {
            libc::prctl(libc::PR_CAPBSET_READ, cap, 0, 0, 0)
        };
        if ret == 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("capability {cap} still in bounding set after drop"),
            ));
        }
    }
    caps::clear(None, caps::CapSet::Inheritable)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    caps::clear(None, caps::CapSet::Permitted)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    caps::clear(None, caps::CapSet::Effective)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn drop_all_capabilities() -> Result<(), std::io::Error> {
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn set_rlimits() -> Result<(), std::io::Error> {
    let nofile = libc::rlimit {
        rlim_cur: 1024,
        rlim_max: 1024,
    };
    let ret = unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &nofile) };
    if ret != 0 {
        return Err(std::io::Error::last_os_error());
    }

    let nproc = libc::rlimit {
        rlim_cur: 64,
        rlim_max: 64,
    };
    let ret = unsafe { libc::setrlimit(libc::RLIMIT_NPROC, &nproc) };
    if ret != 0 {
        return Err(std::io::Error::last_os_error());
    }

    let fsize = libc::rlimit {
        rlim_cur: 10 * 1024 * 1024,
        rlim_max: 10 * 1024 * 1024,
    };
    let ret = unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &fsize) };
    if ret != 0 {
        return Err(std::io::Error::last_os_error());
    }

    let as_limit = libc::rlimit {
        rlim_cur: 512 * 1024 * 1024,
        rlim_max: 512 * 1024 * 1024,
    };
    let ret = unsafe { libc::setrlimit(libc::RLIMIT_AS, &as_limit) };
    if ret != 0 {
        return Err(std::io::Error::last_os_error());
    }

    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn set_rlimits() -> Result<(), std::io::Error> {
    Ok(())
}
