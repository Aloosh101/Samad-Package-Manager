use std::ffi::CString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::error::{SpmError, SpmResult};

pub fn daemon_reload() {
    // Signal systemd to reload unit files by sending SIGHUP to PID 1.
    // This is the same underlying mechanism systemctl uses.
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(1),
        nix::sys::signal::Signal::SIGHUP,
    );
}

fn read_etc_passwd() -> SpmResult<String> {
    Ok(fs::read_to_string("/etc/passwd").unwrap_or_default())
}

fn read_etc_group() -> SpmResult<String> {
    Ok(fs::read_to_string("/etc/group").unwrap_or_default())
}

fn next_gid() -> SpmResult<u32> {
    let group = read_etc_group()?;
    let max_gid = group
        .lines()
        .filter_map(|line| line.split(':').nth(2))
        .filter_map(|gid_str| gid_str.parse::<u32>().ok())
        .max()
        .unwrap_or(999);
    Ok(max_gid + 1)
}

fn next_uid() -> SpmResult<u32> {
    let passwd = read_etc_passwd()?;
    let max_uid = passwd
        .lines()
        .filter_map(|line| line.split(':').nth(2))
        .filter_map(|uid_str| uid_str.parse::<u32>().ok())
        .max()
        .unwrap_or(999);
    Ok(max_uid + 1)
}

pub fn create_sysuser(
    username: &str,
    uid: Option<u32>,
    gid: Option<u32>,
    _groups: &[String],
) -> SpmResult<()> {
    let passwd = read_etc_passwd()?;
    if passwd.lines().any(|line| line.split(':').next() == Some(username)) {
        tracing::debug!("User '{}' already exists, skipping", username);
        return Ok(());
    }

    let group_content = read_etc_group()?;
    let gid = gid.unwrap_or_else(|| next_gid().unwrap_or(1000));
    let uid = uid.unwrap_or_else(|| next_uid().unwrap_or(1000));

    if !group_content
        .lines()
        .any(|line| line.split(':').next() == Some(username))
    {
        let group_line = format!("{}:x:{}:\n", username, gid);
        fs::write("/etc/group", format!("{}{}", group_content.trim_end(), group_line))
            .map_err(|e| SpmError::other(format!("Failed to write /etc/group: {e}")))?;
    }

    let passwd_line = format!("{}:x:{}:{}::/:/sbin/nologin\n", username, uid, gid);
    fs::write(
        "/etc/passwd",
        format!("{}{}", passwd.trim_end(), passwd_line),
    )
    .map_err(|e| SpmError::other(format!("Failed to write /etc/passwd: {e}")))?;

    Ok(())
}

pub fn create_tmpfile(path: &Path, mode: u32, owner: &str) -> SpmResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let file = fs::File::create(path)
        .map_err(|e| SpmError::other(format!("Failed to create tmpfile {}: {e}", path.display())))?;
    file.set_permissions(fs::Permissions::from_mode(mode))
        .map_err(|e| SpmError::other(format!("Failed to set permissions on {}: {e}", path.display())))?;
    drop(file);

    let (uid_val, gid_val) = if let Some((user, group)) = owner.split_once(':') {
        let u = lookup_user(user)?;
        let g = if !group.is_empty() {
            lookup_group(group)?
        } else {
            lookup_user_group(user)?
        };
        (u, g)
    } else {
        let u = lookup_user(owner)?;
        let g = lookup_user_group(owner)?;
        (u, g)
    };

    // Open with O_NOFOLLOW to prevent symlink attacks, then fchown
    let cpath = CString::new(path.to_string_lossy().as_ref())
        .map_err(|e| SpmError::other(format!("Invalid path: {e}")))?;
    let fd = unsafe {
        libc::open(cpath.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW)
    };
    if fd < 0 {
        return Err(SpmError::other(format!(
            "Failed to open {} for ownership change: {}",
            path.display(),
            std::io::Error::last_os_error()
        )));
    }
    let rc = unsafe { libc::fchown(fd, uid_val, gid_val) };
    unsafe { libc::close(fd) };
    if rc != 0 {
        return Err(SpmError::other(format!(
            "Failed to change ownership of {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        )));
    }

    Ok(())
}

fn lookup_user(user: &str) -> SpmResult<u32> {
    let passwd = read_etc_passwd()?;
    for line in passwd.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() >= 3 && parts[0] == user {
            return parts[2]
                .parse::<u32>()
                .map_err(|e| SpmError::other(format!("Invalid UID for {user}: {e}")));
        }
    }
    Err(SpmError::other(format!("User '{user}' not found")))
}

fn lookup_group(group: &str) -> SpmResult<u32> {
    let group_content = read_etc_group()?;
    for line in group_content.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() >= 3 && parts[0] == group {
            return parts[2]
                .parse::<u32>()
                .map_err(|e| SpmError::other(format!("Invalid GID for {group}: {e}")));
        }
    }
    Err(SpmError::other(format!("Group '{group}' not found")))
}

fn lookup_user_group(user: &str) -> SpmResult<u32> {
    let passwd = read_etc_passwd()?;
    for line in passwd.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() >= 4 && parts[0] == user {
            return parts[3]
                .parse::<u32>()
                .map_err(|e| SpmError::other(format!("Invalid GID for {user}: {e}")));
        }
    }
    Err(SpmError::other(format!("User '{user}' not found")))
}
