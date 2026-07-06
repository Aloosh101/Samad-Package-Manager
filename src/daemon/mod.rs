use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::net::UnixListener;

use crate::db;
use crate::error::{SpmError, SpmResult};

pub mod conflict;
pub mod ipc;
pub mod monitor;
pub mod permission;
pub mod store;
pub mod transaction;

pub fn socket_path() -> String {
    match std::env::var("SPM_ROOT") {
        Ok(root) if !root.is_empty() => format!("{}/run/spm.sock", root.trim_end_matches('/')),
        _ => "/run/spm.sock".to_string(),
    }
}
const SPM_GROUP: &str = "spm";
const MAX_CONCURRENT_PER_USER: u32 = 3;
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(1);
const RATE_LIMIT_MAX: u32 = 10;
const AUTO_UPDATE_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Deserialize)]
struct DaemonRequest {
    action: String,
    package: Option<String>,
    user_id: u32,
}

#[derive(Debug, Serialize)]
struct DaemonResponse {
    status: String,
    message: String,
}

impl DaemonResponse {
    fn ok(msg: &str) -> String {
        serde_json::to_string(&DaemonResponse {
            status: "ok".to_string(),
            message: msg.to_string(),
        })
        .unwrap_or_else(|_| r#"{"status":"error","message":"serialization failed"}"#.to_string())
    }

    fn error(msg: &str) -> String {
        serde_json::to_string(&DaemonResponse {
            status: "error".to_string(),
            message: msg.to_string(),
        })
        .unwrap_or_else(|_| r#"{"status":"error","message":"serialization failed"}"#.to_string())
    }
}

struct RateLimiter {
    windows: Mutex<HashMap<u32, (Instant, u32)>>,
    concurrent: Mutex<HashMap<u32, u32>>,
}

impl RateLimiter {
    fn new() -> Self {
        Self {
            windows: Mutex::new(HashMap::new()),
            concurrent: Mutex::new(HashMap::new()),
        }
    }

    fn try_acquire(&self, uid: u32) -> Result<(), String> {
        // Rate limit check first (no side effects to clean up)
        let mut wins = self.windows.lock().map_err(|e| e.to_string())?;
        let now = Instant::now();
        let entry = wins.entry(uid).or_insert((now, 0));
        if now.duration_since(entry.0) > RATE_LIMIT_WINDOW {
            *entry = (now, 1);
        } else if entry.1 >= RATE_LIMIT_MAX {
            return Err(format!("Rate limit exceeded for user {uid} (max {RATE_LIMIT_MAX}/s)"));
        } else {
            entry.1 += 1;
        }
        drop(wins);

        // Concurrent limit check second
        let mut conc = self.concurrent.lock().map_err(|e| e.to_string())?;
        let count = conc.entry(uid).or_insert(0);
        if *count >= MAX_CONCURRENT_PER_USER {
            return Err(format!("Too many concurrent requests for user {uid} (max {MAX_CONCURRENT_PER_USER})"));
        }
        *count += 1;
        Ok(())
    }

    fn release(&self, uid: u32) {
        if let Ok(mut conc) = self.concurrent.lock() {
            if let Some(count) = conc.get_mut(&uid) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    conc.remove(&uid);
                }
            }
        }
    }

} // Close impl RateLimiter

#[cfg(unix)]
fn signal() -> SpmResult<()> {
    tracing::info!("Registered signal handlers via tokio");

    // Spawn a task to listen for signals
    tokio::spawn(async {
        use tokio::signal::unix::{signal, SignalKind};

        let mut sigint = signal(SignalKind::interrupt())
            .expect("Failed to register SIGINT handler");
        let mut sigterm = signal(SignalKind::terminate())
            .expect("Failed to register SIGTERM handler");
        let mut sighup = signal(SignalKind::hangup())
            .expect("Failed to register SIGHUP handler");

        loop {
            tokio::select! {
                _ = sigint.recv() => {
                    tracing::info!("SIGINT received, shutting down...");
                    break;
                }
                _ = sigterm.recv() => {
                    tracing::info!("SIGTERM received, shutting down...");
                    break;
                }
                _ = sighup.recv() => {
                    // SIGHUP — ignore on terminal session end so daemon survives
                    tracing::debug!("SIGHUP received (ignored)");
                }
            }
        }

        tracing::info!("Cleaning up...");
        let _ = std::fs::remove_file(socket_path());
        std::process::exit(0);
    });

    Ok(())
}

impl RateLimiter {
    async fn handle_connection(
        &self,
        stream: UnixStream,
    ) -> SpmResult<()> {
        let (uid, _gid) = peer_cred(&stream)?;

        // Determine authorization ONCE from peer credentials (kernel-provided, no TOCTOU)
        let authorized = is_authorized(uid);

        if let Err(msg) = self.try_acquire(uid) {
            let resp = DaemonResponse::error(&msg);
            let mut w = stream;
            let _ = w.write_all(resp.as_bytes());
            return Err(SpmError::permission_denied(msg));
        }

        let result = self.handle_inner(stream, uid, authorized).await;
        self.release(uid);
        result
    }

    async fn handle_inner(
        &self,
        stream: UnixStream,
        uid: u32,
        authorized: bool,
    ) -> SpmResult<()> {
        let mut reader = BufReader::new(&stream);
        let mut line = String::new();
        reader.read_line(&mut line)
            .map_err(|e| SpmError::other(format!("Cannot read request: {e}")))?;

        let req: DaemonRequest = match serde_json::from_str(line.trim()) {
            Ok(r) => r,
            Err(e) => {
                let resp = DaemonResponse::error(&format!("Invalid JSON: {e}"));
                let mut w = &stream;
                let _ = w.write_all(resp.as_bytes());
                return Err(SpmError::invalid_format(format!("Bad request: {e}")));
            }
        };

        if req.user_id != uid {
            let resp = DaemonResponse::error("user_id mismatch: cannot impersonate another user");
            let mut w = &stream;
            let _ = w.write_all(resp.as_bytes());
            return Err(SpmError::permission_denied(format!(
                "Peer UID {uid} attempted to impersonate UID {}", req.user_id
            )));
        }

        let action = req.action.clone();

        // Set up progress streaming via channel
        let (progress_tx, progress_rx) = mpsc::channel::<String>();
        let writer = stream.try_clone()
            .map_err(|e| SpmError::other(format!("Cannot clone socket: {e}")))?;
        let relay = std::thread::spawn(move || {
            while let Ok(msg) = progress_rx.recv() {
                let line = format!("{{\"type\":\"progress\",\"message\":\"{}\"}}\n", msg);
                let mut w = &writer;
                let _ = w.write_all(line.as_bytes());
                let _ = w.flush();
            }
        });

        crate::output::set_progress_tx(uid, progress_tx.clone());
        crate::output::set_current_uid(uid);

        let response = match action.as_str() {
            "install" => {
                if authorized {
                    handle_system_install(&req, uid).await
                } else {
                    handle_user_install(&req, uid).await
                }
            }
            "remove" => {
                if authorized {
                    handle_system_remove(&req, uid).await
                } else {
                    handle_user_remove(&req, uid).await
                }
            }
            "list" => handle_list(&req, uid, authorized).await,
            "upgrade" => {
                if authorized {
                    handle_upgrade(&req, uid).await
                } else {
                    Err(SpmError::permission_denied("Only root or 'spm' group can upgrade system packages"))
                }
            }
            "update" => {
                if authorized {
                    handle_update().await
                } else {
                    Err(SpmError::permission_denied("Only root or 'spm' group can update repositories"))
                }
            }
            "dist-upgrade" => {
                if authorized {
                    handle_dist_upgrade(&req).await
                } else {
                    Err(SpmError::permission_denied("Only root or 'spm' group can run dist-upgrade"))
                }
            }
            "purge" => {
                if authorized {
                    handle_purge(&req).await
                } else {
                    Err(SpmError::permission_denied("Only root or 'spm' group can purge packages"))
                }
            }
            "autoremove" => {
                if authorized {
                    handle_autoremove(&req).await
                } else {
                    Err(SpmError::permission_denied("Only root or 'spm' group can autoremove packages"))
                }
            }
            "cleanup" => {
                if authorized {
                    handle_cleanup().await
                } else {
                    Err(SpmError::permission_denied("Only root or 'spm' group can run cleanup"))
                }
            }
            "repo" => {
                if authorized {
                    handle_repo(&req).await
                } else {
                    Err(SpmError::permission_denied("Only root or 'spm' group can manage repositories"))
                }
            }
            "snapshot" => {
                if authorized {
                    handle_snapshot(&req).await
                } else {
                    Err(SpmError::permission_denied("Only root or 'spm' group can manage snapshots"))
                }
            }
            other => Err(SpmError::other(format!("Unknown action: {other}"))),
        };

        // Close the progress channel so the relay thread stops
        drop(progress_tx);
        crate::output::clear_progress_tx(uid);
        crate::output::clear_current_uid();
        let _ = relay.join();

        let (status, detail) = match response {
            Ok(msg) => ("ok", msg),
            Err(e) => ("error", e.to_string()),
        };

        let resp = if status == "ok" {
            DaemonResponse::ok(&detail)
        } else {
            DaemonResponse::error(&detail)
        };

        let mut w = &stream;
        let _ = w.write_all(resp.as_bytes());
        if status == "ok" {
            tracing::info!("action={} uid={} ok", action, uid);
        } else {
            tracing::warn!("action={} uid={} error: {}", action, uid, detail);
        }
        Ok(())
    }
}

async fn handle_system_install(
    req: &DaemonRequest,
    _uid: u32,
) -> Result<String, SpmError> {
    let package = req.package.as_deref().ok_or_else(|| {
        SpmError::other("Missing 'package' field".to_string())
    })?;

    let name = package.to_string();
    tokio::task::spawn_blocking(move || -> SpmResult<()> {
        crate::package::install::install_package(&name, None, false, true, false, Default::default())
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Install failed: {e}")))?;

    Ok(format!("Installed {package} (system)"))
}

async fn handle_system_remove(
    req: &DaemonRequest,
    _uid: u32,
) -> Result<String, SpmError> {
    let package = req.package.as_deref().ok_or_else(|| {
        SpmError::other("Missing 'package' field".to_string())
    })?;

    let name = package.to_string();
    tokio::task::spawn_blocking(move || -> SpmResult<()> {
        crate::package::install::remove_package(&name)
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Remove failed: {e}")))?;

    Ok(format!("Removed {package} (system)"))
}

async fn handle_user_install(
    req: &DaemonRequest,
    uid: u32,
) -> Result<String, SpmError> {
    let package = req.package.as_deref().ok_or_else(|| {
        SpmError::other("Missing 'package' field".to_string())
    })?;

    let user_name = crate::util::user::resolve_user_name(uid).unwrap_or_else(|| uid.to_string());
    let user_home = resolve_user_home(uid)?;
    let name = package.to_string();
    tokio::task::spawn_blocking(move || -> SpmResult<()> {
        crate::package::install::install_for_user(&name, uid, &user_home)
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("User install failed: {e}")))?;

    Ok(format!("Installed {package} for {user_name}"))
}

async fn handle_user_remove(
    req: &DaemonRequest,
    uid: u32,
) -> Result<String, SpmError> {
    let package = req.package.as_deref().ok_or_else(|| {
        SpmError::other("Missing 'package' field".to_string())
    })?;

    let user_name = crate::util::user::resolve_user_name(uid).unwrap_or_else(|| uid.to_string());
    let user_home = resolve_user_home(uid)?;
    let name = package.to_string();
    tokio::task::spawn_blocking(move || -> SpmResult<()> {
        crate::package::install::remove_for_user(&name, uid, &user_home)
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("User remove failed: {e}")))?;

    Ok(format!("Removed {package} for {user_name}"))
}

async fn handle_list(
    _req: &DaemonRequest,
    uid: u32,
    authorized: bool,
) -> Result<String, SpmError> {
    if uid == 0 || authorized {
        let packages = db::with_read_lock(|conn| {
            db::list_installed_packages(conn)
        }).map_err(|e| SpmError::other(format!("DB error: {e}")))?;

        let names: Vec<String> = packages.iter().map(|p| {
            format!("{} ({})", p.name, p.format)
        }).collect();

        serde_json::to_string(&names)
            .map_err(|e| SpmError::other(format!("Serialization error: {e}")))
    } else {
        let installs = db::with_read_lock(|conn| {
            db::list_user_installs(conn, uid)
        }).map_err(|e| SpmError::other(format!("DB error: {e}")))?;

        serde_json::to_string(&installs)
            .map_err(|e| SpmError::other(format!("Serialization error: {e}")))
    }
}

async fn handle_upgrade(
    req: &DaemonRequest,
    _uid: u32,
) -> Result<String, SpmError> {
    let package = req.package.clone();
    let is_specific = package.is_some();
    tokio::task::spawn_blocking(move || -> SpmResult<()> {
        crate::package::install::upgrade_package(package.as_deref(), Default::default(), None)
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Upgrade failed: {e}")))?;

    if is_specific {
        Ok("Upgrade check completed".to_string())
    } else {
        Ok("System upgrade completed".to_string())
    }
}

fn resolve_user_home(uid: u32) -> SpmResult<String> {
    if uid == 0 {
        return Ok("/root".to_string());
    }
    use nix::unistd::{Uid, User};
    let user = User::from_uid(Uid::from_raw(uid))
        .map_err(|e| SpmError::other(format!("Cannot resolve user {uid}: {e}")))?
        .ok_or_else(|| SpmError::other(format!("User {uid} not found")))?;
    Ok(user.dir.to_string_lossy().into_owned())
}

fn resolve_group_gid(name: &str) -> Option<u32> {
    use nix::unistd::Group;
    let group = Group::from_name(name).ok()??;
    Some(group.gid.as_raw())
}

fn peer_cred(stream: &UnixStream) -> SpmResult<(u32, u32)> {
    use std::os::unix::io::AsRawFd;
    let fd = stream.as_raw_fd();
    let mut ucred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut ucred as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    if rc != 0 {
        return Err(SpmError::other("Failed to get peer credentials".to_string()));
    }
    Ok((ucred.uid, ucred.gid))
}

fn is_authorized(uid: u32) -> bool {
    if uid == 0 {
        return true;
    }
    let spm_gid = resolve_group_gid(SPM_GROUP);
    let Some(target_gid) = spm_gid else {
        return false;
    };
    // Look up username for this uid
    let username = match resolve_user_name(uid) {
        Some(n) => n,
        None => return false,
    };
    // Call getgrouplist(3) to get all supplementary groups
    let c_username = std::ffi::CString::new(username.as_str()).unwrap();
    let mut ngroups: libc::c_int = 0;
    // First call to get the size
    let ret = unsafe {
        libc::getgrouplist(c_username.as_ptr(), target_gid, std::ptr::null_mut(), &mut ngroups)
    };
    if ret == -1 && ngroups > 0 {
        // Second call with proper buffer
        let mut groups: Vec<libc::gid_t> = vec![0; ngroups as usize];
        let ret = unsafe {
            libc::getgrouplist(c_username.as_ptr(), target_gid, groups.as_mut_ptr(), &mut ngroups)
        };
        if ret != -1 {
            for &g in &groups {
                if g == target_gid {
                    return true;
                }
            }
        }
    }
    // Also check primary group via reentrant API
    let primary_gid = match nix::unistd::User::from_name(&username) {
        Ok(Some(user)) => user.gid.as_raw(),
        _ => return false,
    };
    primary_gid == target_gid
}

fn resolve_user_name(uid: u32) -> Option<String> {
    crate::util::user::resolve_user_name(uid)
}

async fn handle_update() -> Result<String, SpmError> {
    tokio::task::spawn_blocking(move || -> SpmResult<()> {
        crate::config::repos::update_repos()
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Update failed: {e}")))?;
    Ok("Repositories updated".to_string())
}

async fn handle_dist_upgrade(req: &DaemonRequest) -> Result<String, SpmError> {
    let yes = req.package.as_deref().and_then(|v| v.parse::<bool>().ok()).unwrap_or(false);
    tokio::task::spawn_blocking(move || -> SpmResult<()> {
        crate::package::install::dist_upgrade_packages(yes)
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Dist-upgrade failed: {e}")))?;
    Ok("Distribution upgrade completed".to_string())
}

async fn handle_purge(req: &DaemonRequest) -> Result<String, SpmError> {
    let package = req.package.as_deref().ok_or_else(|| {
        SpmError::other("Missing 'package' field".to_string())
    })?;
    let name = package.to_string();
    tokio::task::spawn_blocking(move || -> SpmResult<()> {
        crate::package::install::purge_package(&name)
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Purge failed: {e}")))?;
    Ok(format!("Purged {package}"))
}

async fn handle_autoremove(req: &DaemonRequest) -> Result<String, SpmError> {
    let yes = req.package.as_deref().and_then(|v| v.parse::<bool>().ok()).unwrap_or(false);
    tokio::task::spawn_blocking(move || -> SpmResult<()> {
        crate::package::install::autoremove_packages(yes)
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Autoremove failed: {e}")))?;
    Ok("Autoremove completed".to_string())
}

async fn handle_cleanup() -> Result<String, SpmError> {
    tokio::task::spawn_blocking(move || -> SpmResult<()> {
        crate::package::cleanup::cleanup_all(true)
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Cleanup failed: {e}")))?;
    Ok("Cleanup completed".to_string())
}

async fn handle_repo(req: &DaemonRequest) -> Result<String, SpmError> {
    let detail_str = req.package.as_deref().ok_or_else(|| {
        SpmError::other("Missing repo detail".to_string())
    })?;
    let detail: serde_json::Value = serde_json::from_str(detail_str)
        .map_err(|e| SpmError::other(format!("Invalid repo detail JSON: {e}")))?;

    let action = detail["action"].as_str().unwrap_or("");
    match action {
        "add" => {
            let name = detail["name"].as_str().ok_or_else(|| SpmError::other("Missing 'name'"))?;
            let source_str = detail["source"].as_str().unwrap_or("native");
            let source = match source_str {
                "deb" => crate::types::RepoSource::Deb,
                "rpm" => crate::types::RepoSource::Rpm,
                "native" => crate::types::RepoSource::Native,
                _ => return Err(SpmError::other(format!("Invalid repo source '{source_str}'"))),
            };
            let url = detail["url"].as_str().map(|s| s.to_string());
            let mirrors: Option<Vec<String>> = detail["mirrors"].as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect());
            let mirrors_vec = mirrors.unwrap_or_default();
            let mirrors_opt = if mirrors_vec.is_empty() { None } else { Some(mirrors_vec) };
            let priority = detail["priority"].as_u64().map(|p| p as u32);
            let name_owned = name.to_string();
            tokio::task::spawn_blocking(move || -> SpmResult<()> {
                crate::config::repos::add_repo(&name_owned, source, url, mirrors_opt, priority)
            }).await
                .map_err(|e| SpmError::other(format!("Join error: {e}")))?
                .map_err(|e| SpmError::other(format!("Add repo failed: {e}")))?;
            Ok(format!("Added repository '{name}'"))
        }
        "remove" => {
            let name = detail["name"].as_str().ok_or_else(|| SpmError::other("Missing 'name'"))?;
            let name_owned = name.to_string();
            tokio::task::spawn_blocking(move || -> SpmResult<()> {
                crate::config::repos::remove_repo(&name_owned)
            }).await
                .map_err(|e| SpmError::other(format!("Join error: {e}")))?
                .map_err(|e| SpmError::other(format!("Remove repo failed: {e}")))?;
            Ok(format!("Removed repository '{name}'"))
        }
        "list" => {
            tokio::task::spawn_blocking(move || -> SpmResult<String> {
                // Just verify repos can be listed
                let _ = crate::config::repos::load_repos()?;
                Ok("Repository list available".to_string())
            }).await
                .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        }
        _ => Err(SpmError::other(format!("Unknown repo action: {action}"))),
    }
}

async fn handle_snapshot(req: &DaemonRequest) -> Result<String, SpmError> {
    let detail_str = req.package.as_deref().ok_or_else(|| {
        SpmError::other("Missing snapshot detail".to_string())
    })?;
    let detail: serde_json::Value = serde_json::from_str(detail_str)
        .map_err(|e| SpmError::other(format!("Invalid snapshot detail JSON: {e}")))?;

    let action = detail["action"].as_str().unwrap_or("");
    match action {
        "create" => {
            tokio::task::spawn_blocking(move || -> SpmResult<()> {
                crate::package::query::create_snapshot()
            }).await
                .map_err(|e| SpmError::other(format!("Join error: {e}")))?
                .map_err(|e| SpmError::other(format!("Snapshot creation failed: {e}")))?;
            Ok("Snapshot created".to_string())
        }
        "rollback" => {
            let id = detail["id"].as_str().ok_or_else(|| SpmError::other("Missing 'id' for rollback"))?;
            let id_owned = id.to_string();
            tokio::task::spawn_blocking(move || -> SpmResult<()> {
                crate::package::query::rollback_snapshot(&id_owned)
            }).await
                .map_err(|e| SpmError::other(format!("Join error: {e}")))?
                .map_err(|e| SpmError::other(format!("Snapshot rollback failed: {e}")))?;
            Ok(format!("Rolled back snapshot {id}"))
        }
        _ => Err(SpmError::other(format!("Unknown snapshot action: {action}"))),
    }
}

async fn auto_update_loop() {
    let interval_secs = crate::types::SpmConfig::load()
        .ok()
        .and_then(|c| c.auto_update_interval)
        .unwrap_or(AUTO_UPDATE_INTERVAL.as_secs());
    let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
    // Skip the immediate first tick (run after the first interval)
    interval.tick().await;
    loop {
        interval.tick().await;
        tracing::info!("Auto-update: refreshing repository metadata...");
        if let Err(e) = crate::config::repos::update_repos() {
            tracing::warn!("Auto-update failed: {e}");
        } else {
            tracing::info!("Auto-update completed successfully");
        }
    }
}

/// Try to get a listener from systemd socket activation (LISTEN_FDS / LISTEN_PID).
/// Returns `Some(listener)` if activated by systemd, `None` if we should bind ourselves.
fn try_systemd_socket_activation() -> Option<std::os::unix::net::UnixListener> {
    let listen_pid = std::env::var("LISTEN_PID").ok()?;
    let listen_fds = std::env::var("LISTEN_FDS").ok()?;

    let current_pid = std::process::id().to_string();
    if listen_pid != current_pid {
        return None;
    }

    let fds: u32 = listen_fds.parse().ok()?;
    if fds == 0 {
        return None;
    }

    // SD_LISTEN_FDS_START is always 3
    // Verify fd 3 is a Unix socket before taking ownership
    let mut sock_type: libc::c_int = 0;
    let mut sock_len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            3,
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            &mut sock_type as *mut _ as *mut libc::c_void,
            &mut sock_len,
        )
    };
    if rc != 0 || sock_type != libc::SOCK_STREAM {
        tracing::error!("fd 3 is not a stream socket; systemd socket activation rejected");
        return None;
    }

    use std::os::unix::io::FromRawFd;
    // SAFETY: systemd passes the fd at SD_LISTEN_FDS_START.
    // We verified fd 3 is a SOCK_STREAM Unix socket.
    // We take ownership of the fd — systemd expects this.
    let std_listener = unsafe { std::os::unix::net::UnixListener::from_raw_fd(3) };

    // Unset the env vars so child processes don't inherit them
    std::env::remove_var("LISTEN_PID");
    std::env::remove_var("LISTEN_FDS");

    tracing::info!("spmd using systemd socket activation");
    Some(std_listener)
}

pub async fn run_daemon() -> SpmResult<()> {
    #[cfg(unix)]
    signal()?;

    // Spawn periodic repo update task
    tokio::spawn(async {
        auto_update_loop().await;
    });

    let listener = if let Some(std_listener) = try_systemd_socket_activation() {
        tokio::net::UnixListener::from_std(std_listener)
            .map_err(|e| SpmError::other(format!("Cannot convert systemd socket: {e}")))?
    } else {
        let sock = socket_path();
        // Remove old socket if present
        let _ = std::fs::remove_file(&sock);

        let l = UnixListener::bind(&sock)
            .map_err(|e| SpmError::other(format!("Cannot bind to {sock}: {e}")))?;

        // Only spm group members can connect (authorization is via peer creds)
        std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o660))
            .map_err(|e| SpmError::other(format!("Cannot set socket permissions: {e}")))?;

        l
    };

    tracing::info!("spmd listening on {}", socket_path());

    let limiter = Arc::new(RateLimiter::new());

    loop {
        match listener.accept().await {
            Ok((tokio_stream, _addr)) => {
                let stream = tokio_stream.into_std()
                    .map_err(|e| SpmError::other(format!("Cannot convert to std stream: {e}")))?;
                let l = Arc::clone(&limiter);
                tokio::spawn(async move {
                    if let Err(e) = l.handle_connection(stream).await {
                        tracing::warn!("Connection handler error: {e}");
                    }
                });
            }
            Err(e) => {
                tracing::error!("Accept error: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_daemon_response_ok() {
        let s = DaemonResponse::ok("installed");
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["message"], "installed");
    }

    #[test]
    fn test_daemon_response_error() {
        let s = DaemonResponse::error("not found");
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["status"], "error");
        assert_eq!(v["message"], "not found");
    }

    #[test]
    fn test_daemon_response_ok_escapes_json() {
        // message containing quotes should still produce valid JSON
        let s = DaemonResponse::ok("package \"foo\" installed");
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["status"], "ok");
    }

    #[test]
    fn test_rate_limiter_acquire_release() {
        let limiter = RateLimiter::new();
        // First acquire should succeed
        assert!(limiter.try_acquire(1000).is_ok());
        limiter.release(1000);
    }

    #[test]
    fn test_rate_limiter_concurrent_limit() {
        let limiter = RateLimiter::new();
        // Acquire up to MAX_CONCURRENT_PER_USER (3)
        for _ in 0..MAX_CONCURRENT_PER_USER {
            assert!(limiter.try_acquire(1001).is_ok());
        }
        // Next should fail
        let result = limiter.try_acquire(1001);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Too many concurrent"));
    }

    #[test]
    fn test_rate_limiter_release_frees_slot() {
        let limiter = RateLimiter::new();
        for _ in 0..MAX_CONCURRENT_PER_USER {
            limiter.try_acquire(1002).unwrap();
        }
        assert!(limiter.try_acquire(1002).is_err());
        limiter.release(1002);
        // After release, should be able to acquire again
        assert!(limiter.try_acquire(1002).is_ok());
        limiter.release(1002);
    }

    #[test]
    fn test_rate_limiter_different_users_independent() {
        let limiter = RateLimiter::new();
        // Fill up user 1
        for _ in 0..MAX_CONCURRENT_PER_USER {
            limiter.try_acquire(2000).unwrap();
        }
        assert!(limiter.try_acquire(2000).is_err());
        // User 2 should still be able to acquire
        assert!(limiter.try_acquire(2001).is_ok());
        // Release user 1
        limiter.release(2000);
        assert!(limiter.try_acquire(2000).is_ok());
        limiter.release(2000);
        limiter.release(2001);
    }

    #[test]
    fn test_rate_limiter_release_nonexistent() {
        let limiter = RateLimiter::new();
        // Releasing a user that never acquired should not panic
        limiter.release(9999);
    }
}
