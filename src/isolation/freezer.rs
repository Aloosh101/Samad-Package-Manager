use std::fs;
use std::time::Duration;

use crate::error::{SpmError, SpmResult};

#[derive(Debug)]
pub struct ProcessFreezer;

impl ProcessFreezer {
    pub fn new() -> Self {
        Self
    }

    pub fn freeze(pid: u32) -> SpmResult<()> {
        if !Self::can_control(pid)? {
            return Err(SpmError::permission_denied(format!(
                "cannot control process {pid}: not root and not same user"
            )));
        }

        let ret = unsafe { libc::kill(pid as i32, libc::SIGSTOP) };
        if ret != 0 {
            return Err(SpmError::other(format!(
                "SIGSTOP failed for PID {pid}: {}",
                std::io::Error::last_os_error()
            )));
        }

        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(10));
            if Self::is_stopped(pid)? {
                return Ok(());
            }
        }

        Err(SpmError::other(format!(
            "PID {pid} did not stop within timeout"
        )))
    }

    pub fn unfreeze(pid: u32) -> SpmResult<()> {
        let ret = unsafe { libc::kill(pid as i32, libc::SIGCONT) };
        if ret != 0 {
            return Err(SpmError::other(format!(
                "SIGCONT failed for PID {pid}: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(())
    }

    pub fn kill(pid: u32, graceful: bool) -> SpmResult<()> {
        if graceful {
            let ret = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
            if ret != 0 {
                return Err(SpmError::other(format!(
                    "SIGTERM failed for PID {pid}: {}",
                    std::io::Error::last_os_error()
                )));
            }

            for _ in 0..50 {
                std::thread::sleep(Duration::from_millis(100));
                if !Self::is_alive(pid)? {
                    return Ok(());
                }
            }
        }

        let ret = unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        if ret != 0 {
            return Err(SpmError::other(format!(
                "SIGKILL failed for PID {pid}: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(())
    }

    pub fn can_control(pid: u32) -> SpmResult<bool> {
        let uid = unsafe { libc::getuid() };
        if uid == 0 {
            return Ok(true);
        }

        match Self::process_uid(pid) {
            Ok(proc_uid) => Ok(proc_uid == uid),
            Err(_) => Ok(false),
        }
    }

    fn is_stopped(pid: u32) -> SpmResult<bool> {
        let status_path = format!("/proc/{pid}/status");
        let content = match fs::read_to_string(&status_path) {
            Ok(c) => c,
            Err(_) => return Ok(false),
        };
        for line in content.lines() {
            if line.starts_with("State:") {
                return Ok(line.contains('T'));
            }
        }
        Ok(false)
    }

    fn is_alive(pid: u32) -> SpmResult<bool> {
        let stat_path = format!("/proc/{pid}/stat");
        match fs::read_to_string(&stat_path) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(SpmError::Io(e)),
        }
    }

    fn process_uid(pid: u32) -> SpmResult<u32> {
        let status_path = format!("/proc/{pid}/status");
        let content = fs::read_to_string(&status_path)?;
        for line in content.lines() {
            if line.starts_with("Uid:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    return parts[1]
                        .parse::<u32>()
                        .map_err(|e| SpmError::other(format!("invalid Uid in /proc/{pid}/status: {e}")));
                }
            }
        }
        Err(SpmError::other(format!("could not find Uid in /proc/{pid}/status")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new() {
        let freezer = ProcessFreezer::new();
        assert!(format!("{:?}", freezer).contains("ProcessFreezer"));
    }

    #[test]
    fn test_can_control_self() {
        let pid = std::process::id();
        let result = ProcessFreezer::can_control(pid);
        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[test]
    fn test_process_uid_self() {
        let pid = std::process::id();
        let result = ProcessFreezer::process_uid(pid);
        assert!(result.is_ok());
        let uid = unsafe { libc::getuid() };
        assert_eq!(result.unwrap(), uid);
    }

    #[test]
    fn test_is_alive_self() {
        let pid = std::process::id();
        let result = ProcessFreezer::is_alive(pid);
        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[test]
    fn test_is_alive_nonexistent() {
        let result = ProcessFreezer::is_alive(u32::MAX);
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }

    #[test]
    fn test_is_stopped_self() {
        let pid = std::process::id();
        let result = ProcessFreezer::is_stopped(pid);
        assert!(result.is_ok());
        assert!(!result.unwrap(), "current process is not stopped");
    }

}
