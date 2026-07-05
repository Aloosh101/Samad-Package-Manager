use std::os::unix::io::AsRawFd;
use std::path::Path;

use crate::error::{SpmError, SpmResult};

pub fn is_btrfs_subvolume(path: &Path) -> SpmResult<bool> {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mounts = std::fs::read_to_string("/proc/mounts")
        .map_err(|e| crate::error::SpmError::other(format!("Cannot read /proc/mounts: {e}")))?;

    let mut best_match: Option<&str> = None;
    let mut best_len = 0usize;

    for line in mounts.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        let mount_point = parts[1];
        let fs_type = parts[2];

        if fs_type == "btrfs" {
            let mp_normalized = mount_point.trim_end_matches('/');
            let path_str = canonical.to_string_lossy();
            if path_str.starts_with(mp_normalized) && mp_normalized.len() > best_len {
                best_match = Some(mount_point);
                best_len = mp_normalized.len();
            }
        }
    }

    Ok(best_match.is_some())
}

pub fn create_btrfs_snapshot(source: &Path, dest_dir: &Path, name: &str) -> SpmResult<()> {
    let src_fd = std::fs::File::open(source)
        .map_err(|e| SpmError::other(format!("Cannot open source subvolume {}: {e}", source.display())))?;
    let dest_fd = std::fs::File::open(dest_dir)
        .map_err(|e| SpmError::other(format!("Cannot open dest dir {}: {e}", dest_dir.display())))?;

    #[repr(C)]
    struct BtrfsVolArgs {
        fd: i64,
        name: [u8; 4088],
    }

    let mut args = BtrfsVolArgs {
        fd: src_fd.as_raw_fd() as i64,
        name: [0u8; 4088],
    };

    let name_bytes = name.as_bytes();
    let len = name_bytes.len().min(4087);
    args.name[..len].copy_from_slice(&name_bytes[..len]);

    // BTRFS_IOC_SNAP_CREATE = _IOWR(0x94, 1, struct btrfs_ioctl_vol_args)
    const NR: libc::c_ulong = ((3 as libc::c_ulong) << 30)
        | ((4096 as libc::c_ulong) << 16)
        | ((0x94 as libc::c_ulong) << 8)
        | (1 as libc::c_ulong);
    let rc = unsafe {
        libc::syscall(
            libc::SYS_ioctl,
            dest_fd.as_raw_fd() as libc::c_long,
            NR as libc::c_long,
            &args as *const _ as libc::c_long,
        )
    };
    if rc < 0 {
        return Err(SpmError::other(format!(
            "Failed to create btrfs snapshot: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(())
}

pub fn rollback_btrfs_snapshot(snapshot_path: &Path, dest_dir: &Path) -> SpmResult<()> {
    let src_fd = std::fs::File::open(snapshot_path)
        .map_err(|e| SpmError::other(format!("Cannot open snapshot {}: {e}", snapshot_path.display())))?;
    let dest_fd = std::fs::File::open(dest_dir)
        .map_err(|e| SpmError::other(format!("Cannot open dest dir {}: {e}", dest_dir.display())))?;

    #[repr(C)]
    struct BtrfsVolArgs {
        fd: i64,
        name: [u8; 4088],
    }

    let name = "root";
    let mut args = BtrfsVolArgs {
        fd: src_fd.as_raw_fd() as i64,
        name: [0u8; 4088],
    };
    let name_bytes = name.as_bytes();
    args.name[..name_bytes.len()].copy_from_slice(name_bytes);

    const NR: libc::c_ulong = ((3 as libc::c_ulong) << 30)
        | ((4096 as libc::c_ulong) << 16)
        | ((0x94 as libc::c_ulong) << 8)
        | (1 as libc::c_ulong);
    let rc = unsafe {
        libc::syscall(
            libc::SYS_ioctl,
            dest_fd.as_raw_fd() as libc::c_long,
            NR as libc::c_long,
            &args as *const _ as libc::c_long,
        )
    };
    if rc < 0 {
        return Err(SpmError::other(format!(
            "Failed to rollback btrfs snapshot: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(())
}

pub fn list_btrfs_snapshots(subvol_path: &Path) -> SpmResult<Vec<String>> {
    let sysfs_path = Path::new("/sys/fs/btrfs");
    if !sysfs_path.exists() {
        return Ok(Vec::new());
    }

    let mut snapshots = Vec::new();
    if let Ok(entries) = std::fs::read_dir(sysfs_path) {
        for entry in entries.flatten() {
            let fsid_path = entry.path();
            if !fsid_path.is_dir() {
                continue;
            }
            let snapshots_dir = fsid_path.join("snapshots");
            if snapshots_dir.exists() {
                if let Ok(snap_entries) = std::fs::read_dir(&snapshots_dir) {
                    for snap in snap_entries.flatten() {
                        let snap_id = snap.file_name().to_string_lossy().to_string();
                        let snap_info_path = snap.path().join("name");
                        if let Ok(name) = std::fs::read_to_string(&snap_info_path) {
                            let name = name.trim().to_string();
                            if name.contains(&subvol_path.to_string_lossy().as_ref()) {
                                snapshots.push(name);
                            }
                        } else {
                            snapshots.push(snap_id);
                        }
                    }
                }
            }
        }
    }

    Ok(snapshots)
}
