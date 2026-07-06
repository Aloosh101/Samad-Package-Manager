use std::ffi::CString;

use super::MountSpec;

// ═══════════════════════════════════════════════════════════════
// Pre-allocated context for async-signal-safe pre_exec usage.
// All heap data is created BEFORE fork in build_preexec_data().
// preexec_run() only calls async-signal-safe syscalls.
// ═══════════════════════════════════════════════════════════════

#[cfg(target_os = "linux")]
struct MountCStr {
    source: CString,
    target: CString,
    readonly: bool,
}

#[cfg(target_os = "linux")]
pub struct PreExecData {
    root: CString,
    mounts: Vec<MountCStr>,
    need_mount: bool,
    need_user: bool,
    need_net: bool,
    uid_map: CString,
    gid_map: CString,
}

#[cfg(target_os = "linux")]
impl PreExecData {
    pub fn build(mounts: &[MountSpec], need_mount: bool, need_user: bool, need_net: bool) -> Result<Self, std::io::Error> {
        let root = CString::new("/").unwrap();
        let mount_cstrs: Vec<MountCStr> = mounts.iter().map(|spec| {
            Ok(MountCStr {
                source: CString::new(spec.source.as_os_str().as_encoded_bytes())
                    .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid source"))?,
                target: CString::new(spec.target.as_os_str().as_encoded_bytes())
                    .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid target"))?,
                readonly: spec.readonly,
            })
        }).collect::<Result<Vec<_>, std::io::Error>>()?;

        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        let uid_map = CString::new(format!("{uid} {uid} 1\n")).unwrap();
        let gid_map = CString::new(format!("{gid} {gid} 1\n")).unwrap();

        Ok(Self { root, mounts: mount_cstrs, need_mount, need_user, need_net, uid_map, gid_map })
    }
}

/// Call this ONLY from pre_exec (async-signal-safe — no allocation).
/// All CStrings were pre-allocated; this only makes raw syscalls.
#[cfg(target_os = "linux")]
pub unsafe fn preexec_run(data: &PreExecData) -> Result<(), std::io::Error> {
    // ---------- unshare ----------
    let mut flags = libc::CLONE_NEWPID | libc::CLONE_NEWUTS;
    if data.need_mount { flags |= libc::CLONE_NEWNS; }
    if data.need_user { flags |= libc::CLONE_NEWUSER; }
    if data.need_net  { flags |= libc::CLONE_NEWNET; }
    if libc::unshare(flags) != 0 {
        return Err(std::io::Error::last_os_error());
    }

    // ---------- mount namespace ----------
    if data.need_mount {
        if libc::mount(std::ptr::null(), data.root.as_ptr(), std::ptr::null(),
            (libc::MS_PRIVATE | libc::MS_REC) as libc::c_ulong, std::ptr::null()) != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        for m in &data.mounts {
            if libc::mount(m.source.as_ptr(), m.target.as_ptr(), std::ptr::null(),
                libc::MS_BIND as libc::c_ulong, std::ptr::null()) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            if m.readonly {
                if libc::mount(std::ptr::null(), m.target.as_ptr(), std::ptr::null(),
                    (libc::MS_REMOUNT | libc::MS_BIND | libc::MS_RDONLY) as libc::c_ulong, std::ptr::null()) != 0
                {
                    return Err(std::io::Error::last_os_error());
                }
            }
        }
        if libc::mount(std::ptr::null(), data.root.as_ptr(), std::ptr::null(),
            (libc::MS_REMOUNT | libc::MS_RDONLY | libc::MS_BIND) as libc::c_ulong, std::ptr::null()) != 0
        {
            return Err(std::io::Error::last_os_error());
        }
    }

    // ---------- user namespace ----------
    if data.need_user {
        // uid_map
        let uid_fd = libc::open(b"/proc/self/uid_map\0".as_ptr() as *const libc::c_char, libc::O_WRONLY | libc::O_CLOEXEC);
        if uid_fd < 0 { return Err(std::io::Error::last_os_error()); }
        let uid_buf = data.uid_map.as_bytes();
        let written = libc::write(uid_fd, uid_buf.as_ptr() as *const libc::c_void, uid_buf.len());
        if written as usize != uid_buf.len() {
            libc::close(uid_fd);
            return Err(std::io::Error::last_os_error());
        }
        libc::close(uid_fd);

        // setgroups deny
        let sg_fd = libc::open(b"/proc/self/setgroups\0".as_ptr() as *const libc::c_char, libc::O_WRONLY | libc::O_CLOEXEC);
        if sg_fd >= 0 {
            let written = libc::write(sg_fd, b"deny\n" as *const u8 as *const libc::c_void, 5);
            if written != 5 {
                libc::close(sg_fd);
                return Err(std::io::Error::last_os_error());
            }
            libc::close(sg_fd);
        }

        // gid_map
        let gid_fd = libc::open(b"/proc/self/gid_map\0".as_ptr() as *const libc::c_char, libc::O_WRONLY | libc::O_CLOEXEC);
        if gid_fd < 0 { return Err(std::io::Error::last_os_error()); }
        let gid_buf = data.gid_map.as_bytes();
        let written = libc::write(gid_fd, gid_buf.as_ptr() as *const libc::c_void, gid_buf.len());
        if written as usize != gid_buf.len() {
            libc::close(gid_fd);
            return Err(std::io::Error::last_os_error());
        }
        libc::close(gid_fd);
    }

    // ---------- UTS namespace ----------
    let hostname = b"sandbox\0" as *const u8 as *const libc::c_char;
    if libc::sethostname(hostname, 7) != 0 {
        return Err(std::io::Error::last_os_error());
    }

    // ---------- NoNewPrivs ----------
    if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
        return Err(std::io::Error::last_os_error());
    }

    // ---------- drop capabilities ----------
    for cap in 0..64u64 {
        if libc::prctl(libc::PR_CAPBSET_DROP, cap, 0, 0, 0) != 0 {
            if *libc::__errno_location() != libc::EINVAL {
                return Err(std::io::Error::last_os_error());
            }
        }
    }
    for cap in 0..64u64 {
        if libc::prctl(libc::PR_CAPBSET_READ, cap, 0, 0, 0) == 1 {
            return Err(std::io::Error::new(std::io::ErrorKind::Other, "cap still in bounding set"));
        }
    }
    if libc::prctl(libc::PR_SET_KEEPCAPS, 0, 0, 0, 0) != 0 {
        return Err(std::io::Error::last_os_error());
    }

    // ---------- rlimits ----------
    let nofile = libc::rlimit { rlim_cur: 1024, rlim_max: 1024 };
    if libc::setrlimit(libc::RLIMIT_NOFILE, &nofile) != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let nproc = libc::rlimit { rlim_cur: 64, rlim_max: 64 };
    if libc::setrlimit(libc::RLIMIT_NPROC, &nproc) != 0 {
        return Err(std::io::Error::last_os_error());
    }

    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub struct PreExecData;

#[cfg(not(target_os = "linux"))]
pub unsafe fn preexec_run(_data: &PreExecData) -> Result<(), std::io::Error> {
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "sandbox not supported"))
}

// ═══════════════════════════════════════════════════════════════
// Wrapper functions for non-pre_exec use (e.g. daemon sandbox).
// These are safe to allocate since they run in the parent process.
// ═══════════════════════════════════════════════════════════════

#[cfg(target_os = "linux")]
pub fn unshare_namespaces(need_mount: bool, need_user: bool, need_net: bool, need_uts: bool) -> Result<(), std::io::Error> {
    let mut flags = libc::CLONE_NEWPID;
    if need_uts { flags |= libc::CLONE_NEWUTS; }
    if need_mount { flags |= libc::CLONE_NEWNS; }
    if need_user { flags |= libc::CLONE_NEWUSER; }
    if need_net { flags |= libc::CLONE_NEWNET; }
    let ret = unsafe { libc::unshare(flags) };
    if ret != 0 {
        let err = std::io::Error::last_os_error();
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("unshare failed ({flags:#x}): {err}"),
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn unshare_namespaces(_: bool, _: bool, _: bool, _: bool) -> Result<(), std::io::Error> {
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "namespaces not supported"))
}

#[cfg(target_os = "linux")]
pub fn setup_mount_namespace(mounts: &[MountSpec]) -> Result<(), std::io::Error> {
    let data = PreExecData::build(mounts, true, false, false)?;
    unsafe { preexec_run(&data) }
}

#[cfg(target_os = "linux")]
pub fn setup_user_namespace() -> Result<(), std::io::Error> {
    let data = PreExecData::build(&[], false, true, false)?;
    unsafe { preexec_run(&data) }
}

#[cfg(target_os = "linux")]
pub fn setup_uts_namespace() -> Result<(), std::io::Error> {
    let data = PreExecData::build(&[], false, false, false)?;
    unsafe { preexec_run(&data) }
}

#[cfg(target_os = "linux")]
pub fn set_no_new_privs() -> Result<(), std::io::Error> {
    let ret = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
    if ret != 0 { return Err(std::io::Error::last_os_error()); }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn drop_all_capabilities() -> Result<(), std::io::Error> {
    for cap in 0..64u64 {
        let ret = unsafe { libc::prctl(libc::PR_CAPBSET_DROP, cap, 0, 0, 0) };
        if ret != 0 && unsafe { *libc::__errno_location() } != libc::EINVAL {
            return Err(std::io::Error::last_os_error());
        }
    }
    for cap in 0..64u64 {
        if unsafe { libc::prctl(libc::PR_CAPBSET_READ, cap, 0, 0, 0) } == 1 {
            return Err(std::io::Error::new(std::io::ErrorKind::Other, format!("capability {cap} still present")));
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn set_rlimits() -> Result<(), std::io::Error> {
    let nofile = libc::rlimit { rlim_cur: 1024, rlim_max: 1024 };
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &nofile) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let nproc = libc::rlimit { rlim_cur: 64, rlim_max: 64 };
    if unsafe { libc::setrlimit(libc::RLIMIT_NPROC, &nproc) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
