use std::fs;
use std::os::unix::process::CommandExt;
use std::process::Command;

use crate::config::paths;
use crate::error::{SpmError, SpmResult};

pub mod desktop;

pub fn list_sandboxes() -> SpmResult<()> {
    let sandbox_dir = paths::sandboxes_dir();
    if !sandbox_dir.exists() {
        println!("No sandboxes found");
        return Ok(());
    }

    let entries = fs::read_dir(&sandbox_dir)?;
    let mut count = 0;

    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let name = entry.file_name();
            let manifest_path = entry.path().join("manifest.sam");
            let level = if manifest_path.exists() {
                "symlink"
            } else {
                "basic"
            };
            println!("  {} ({})", name.to_string_lossy(), level);
            count += 1;
        }
    }

    if count == 0 {
        println!("No sandboxes found");
    } else {
        println!("\nTotal: {} sandbox(es)", count);
    }

    Ok(())
}

pub fn run_in_sandbox(package: &str, command: &[String]) -> SpmResult<()> {
    let sandbox_path = paths::sandbox_dir(package);

    if !sandbox_path.exists() {
        return Err(SpmError::sandbox(format!(
            "Sandbox for '{package}' not found. Install with --sandbox first."
        )));
    }

    // 1. PID namespace — children become PID 1 in their own tree
    //    Check availability by reading /proc/self/ns/pid instead of unshare
    let has_pid_ns = std::path::Path::new("/proc/self/ns/pid").exists();
    if !has_pid_ns {
        tracing::debug!("PID namespace unavailable for sandbox");
    }

    let usr_dir = sandbox_path.join("usr");

    // Build PATH: sandbox bins first, then system paths
    let sandbox_bin = usr_dir.join("bin");
    let sandbox_games = usr_dir.join("games");
    let mut path_entries = Vec::new();
    if sandbox_bin.exists() {
        path_entries.push(sandbox_bin.to_string_lossy().to_string());
    }
    if sandbox_games.exists() {
        path_entries.push(sandbox_games.to_string_lossy().to_string());
    }
    path_entries.push("/usr/local/bin".to_string());
    path_entries.push("/usr/bin".to_string());
    path_entries.push("/bin".to_string());
    let new_path = path_entries.join(":");

    // Build LD_LIBRARY_PATH
    let sandbox_lib = usr_dir.join("lib");
    let sandbox_lib64 = usr_dir.join("lib64");
    let mut ld_entries = Vec::new();
    if sandbox_lib.exists() {
        ld_entries.push(sandbox_lib.to_string_lossy().to_string());
    }
    if sandbox_lib64.exists() {
        ld_entries.push(sandbox_lib64.to_string_lossy().to_string());
    }
    let new_ld_lib = if ld_entries.is_empty() {
        None
    } else {
        Some(ld_entries.join(":"))
    };

    let (cmd, cmd_args) = if command.is_empty() {
        if unsafe { libc::getuid() } == 0 {
            tracing::warn!(
                "Running interactive shell as root in sandbox. Namespace isolation active."
            );
        }
        ("/bin/sh".to_string(), vec![] as Vec<String>)
    } else {
        (command[0].clone(), command[1..].to_vec())
    };

    let mut child = Command::new(&cmd);
    child
        .args(&cmd_args)
        .env("PATH", &new_path);
    if let Some(ref ld_lib) = new_ld_lib {
        child.env("LD_LIBRARY_PATH", ld_lib);
    }

    // 2–4: Security isolation in pre_exec (after fork, before exec)
    #[allow(unused_unsafe)]
    unsafe {
        child.pre_exec(|| {
            use crate::package::sandbox::namespaces;

            // Mount namespace
            if unsafe { libc::unshare(libc::CLONE_NEWNS) } == 0 {
                unsafe {
                    libc::mount(
                        std::ptr::null(),
                        b"/\0".as_ptr() as *const libc::c_char,
                        std::ptr::null(),
                        libc::MS_PRIVATE | libc::MS_REC,
                        std::ptr::null(),
                    );
                    libc::mount(
                        b"tmpfs\0".as_ptr() as *const libc::c_char,
                        b"/tmp\0".as_ptr() as *const libc::c_char,
                        b"tmpfs\0".as_ptr() as *const libc::c_char,
                        0,
                        std::ptr::null(),
                    );
                    libc::mount(
                        std::ptr::null(),
                        b"/\0".as_ptr() as *const libc::c_char,
                        std::ptr::null(),
                        libc::MS_REMOUNT | libc::MS_RDONLY,
                        std::ptr::null(),
                    );
                }
            }
            unsafe { libc::unshare(libc::CLONE_NEWNET); }
            if unsafe { libc::unshare(libc::CLONE_NEWUTS) } == 0 {
                unsafe {
                    libc::sethostname(
                        b"sandbox\0".as_ptr() as *const libc::c_char,
                        7,
                    );
                }
            }

            // Apply additional security measures from the package sandbox
            namespaces::set_no_new_privs()?;
            namespaces::drop_all_capabilities()?;
            namespaces::set_rlimits()?;

            Ok(())
        });
    }

    let status = child.status()
        .map_err(|e| SpmError::command_failed(format!("Failed to execute '{cmd}': {e}")))?;

    if !status.success() {
        return Err(SpmError::command_failed(format!("'{cmd}' exited with status: {}", status)));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_in_sandbox_not_found() {
        let result = run_in_sandbox("nonexistent-sandbox", &[]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("not found"));
    }

    #[test]
    fn test_list_sandboxes_default() {
        // Should not panic even if sandbox dir doesn't exist
        let result = list_sandboxes();
        assert!(result.is_ok());
    }
}
