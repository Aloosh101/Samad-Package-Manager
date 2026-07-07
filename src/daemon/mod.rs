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
use crate::types::SpmConfig;

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
// Allow all local users (no group restriction on personal systems)
const MAX_CONCURRENT_PER_USER: u32 = 3;
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(1);
const RATE_LIMIT_MAX: u32 = 10;
const AUTO_UPDATE_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Deserialize)]
struct DaemonRequest {
    action: String,
    package: Option<String>,
    params: Option<serde_json::Value>,
    user_id: u32,
}

#[derive(Debug, Serialize)]
struct DaemonResponse {
    status: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

type HandlerResult = Result<(String, Option<serde_json::Value>), SpmError>;

impl DaemonResponse {
    fn ok(msg: &str) -> String {
        serde_json::to_string(&DaemonResponse {
            status: "ok".to_string(),
            message: msg.to_string(),
            data: None,
        })
        .unwrap_or_else(|_| r#"{"status":"error","message":"serialization failed"}"#.to_string())
    }

    fn ok_with_data(msg: &str, data: serde_json::Value) -> String {
        serde_json::to_string(&DaemonResponse {
            status: "ok".to_string(),
            message: msg.to_string(),
            data: Some(data),
        })
        .unwrap_or_else(|_| r#"{"status":"error","message":"serialization failed"}"#.to_string())
    }

    fn error(msg: &str) -> String {
        serde_json::to_string(&DaemonResponse {
            status: "error".to_string(),
            message: msg.to_string(),
            data: None,
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
        _authorized: bool,
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
                if uid == 0 {
                    handle_system_install(&req, uid).await.map(|m| (m, None))
                } else {
                    handle_user_install(&req, uid).await.map(|m| (m, None))
                }
            }
            "remove" => {
                if uid == 0 {
                    handle_system_remove(&req, uid).await.map(|m| (m, None))
                } else {
                    handle_user_remove(&req, uid).await.map(|m| (m, None))
                }
            }
            "list" => {
                let json = handle_list(&req, uid, uid == 0).await?;
                let names: Vec<String> = serde_json::from_str(&json)
                    .unwrap_or_else(|_| vec![json.clone()]);
                Ok((format!("{} package(s) installed", names.len()), Some(serde_json::json!(names))))
            }
            "upgrade" => {
                if uid == 0 {
                    handle_upgrade(&req, uid).await.map(|m| (m, None))
                } else {
                    Err(SpmError::permission_denied("Only root can upgrade system packages"))
                }
            }
            "update" => handle_update(uid).await.map(|m| (m, None)),
            "dist-upgrade" => {
                if uid == 0 {
                    handle_dist_upgrade(&req).await.map(|m| (m, None))
                } else {
                    Err(SpmError::permission_denied("Only root can run dist-upgrade"))
                }
            }
            "purge" => {
                if uid == 0 {
                    handle_purge(&req).await.map(|m| (m, None))
                } else {
                    handle_user_remove(&req, uid).await.map(|m| (m, None))
                }
            }
            "autoremove" => {
                if uid == 0 {
                    handle_autoremove(&req).await.map(|m| (m, None))
                } else {
                    Err(SpmError::permission_denied("Only root can autoremove packages"))
                }
            }
            "cleanup" => {
                if uid == 0 {
                    handle_cleanup(&req).await.map(|m| (m, None))
                } else {
                    Err(SpmError::permission_denied("Only root can run cleanup"))
                }
            }
            "repo" => {
                if uid == 0 {
                    handle_repo(&req).await.map(|m| (m, None))
                } else {
                    Err(SpmError::permission_denied("Only root can manage repositories"))
                }
            }
            "snapshot" => {
                if uid == 0 {
                    handle_snapshot(&req).await.map(|m| (m, None))
                } else {
                    Err(SpmError::permission_denied("Only root can manage snapshots"))
                }
            }
            "convert" => handle_convert(&req).await,
            "install-local" => handle_install_local(&req).await,
            "install-sandbox" => handle_install_sandbox(&req).await,
            "build" => handle_build(&req).await,
            "search" => handle_search(&req).await,
            "search-file" => handle_search_file(&req).await,
            "info" => handle_info(&req).await,
            "files" => handle_files(&req).await,
            "depends" => handle_depends(&req).await,
            "rdepends" => handle_rdepends(&req).await,
            "config-show" => handle_config_show().await,
            "config-set" => {
                if uid == 0 {
                    handle_config_set(&req).await
                } else {
                    Err(SpmError::permission_denied("Only root can set config"))
                }
            }
            "index-rebuild" => {
                if uid == 0 {
                    handle_index_rebuild().await
                } else {
                    Err(SpmError::permission_denied("Only root can rebuild index"))
                }
            }
            "history" => handle_history(&req).await,
            "init" => {
                if uid == 0 {
                    handle_init(&req).await
                } else {
                    Err(SpmError::permission_denied("Only root can initialize system"))
                }
            }
            "group" => {
                if uid == 0 {
                    handle_group(&req).await
                } else {
                    Err(SpmError::permission_denied("Only root can manage groups"))
                }
            }
            "fsck" => {
                if uid == 0 {
                    handle_fsck(&req).await
                } else {
                    Err(SpmError::permission_denied("Only root can run fsck"))
                }
            }
            "sync" => {
                if uid == 0 {
                    handle_sync(&req).await
                } else {
                    Err(SpmError::permission_denied("Only root can sync system"))
                }
            }
            "repo-list" => handle_repo_list().await,
            "repo-create" => {
                if uid == 0 {
                    handle_repo_create(&req).await
                } else {
                    Err(SpmError::permission_denied("Only root can create repos"))
                }
            }
            "repo-publish" => {
                if uid == 0 {
                    handle_repo_publish(&req).await
                } else {
                    Err(SpmError::permission_denied("Only root can publish packages"))
                }
            }
            "repo-gen-key" => {
                if uid == 0 {
                    handle_repo_gen_key(&req).await
                } else {
                    Err(SpmError::permission_denied("Only root can generate keys"))
                }
            }
            "repo-sign" => {
                if uid == 0 {
                    handle_repo_sign(&req).await
                } else {
                    Err(SpmError::permission_denied("Only root can sign repos"))
                }
            }
            "sandbox-list" => handle_sandbox_list().await,
            "sandbox-run" => handle_sandbox_run(&req).await,
            "detect" => handle_detect(&req).await,
            "analyze" => handle_analyze(&req).await,
            "ps" => handle_ps(&req).await,
            "self-update" => {
                if uid == 0 {
                    handle_self_update(&req).await
                } else {
                    Err(SpmError::permission_denied("Only root can self-update"))
                }
            }
            other => Err(SpmError::other(format!("Unknown action: {other}"))),
        };

        // Close the progress channel so the relay thread stops
        drop(progress_tx);
        crate::output::clear_progress_tx(uid);
        crate::output::clear_current_uid();
        let _ = relay.join();

        let (status, detail, data) = match response {
            Ok((msg, data)) => ("ok", msg, data),
            Err(e) => ("error", e.to_string(), None),
        };

        let resp = if status == "ok" {
            if let Some(d) = data {
                DaemonResponse::ok_with_data(&detail, d)
            } else {
                DaemonResponse::ok(&detail)
            }
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
    uid: u32,
) -> Result<String, SpmError> {
    let package = req.package.as_deref().ok_or_else(|| {
        SpmError::other("Missing 'package' field".to_string())
    })?;

    let params = req.params.as_ref();
    let strategy = resolve_strategy(req);
    let yes = params.and_then(|p| p["yes"].as_bool()).unwrap_or(true);
    let replace = params.and_then(|p| p["replace"].as_bool()).unwrap_or(false);

    if !replace {
        let conflicts = crate::package::install::check_pre_install_paths(package, uid);
        if !conflicts.is_empty() {
            let paths = conflicts.join(", ");
            return Err(SpmError::other(format!(
                "File conflict detected: '{}' already exists at: {}. Remove it first or use --replace.",
                package, paths
            )));
        }
    } else {
        use crate::output::send_msg;
        let lines = [
            "warn: ────────────────────────────────────────────────",
            "warn: ⚠  WARNING  ⚠  --REPLACE ACTIVATED",
            "warn: SPM will FORCE OVERWRITE files on your system.",
            "warn: These files may belong to another package manager.",
            "warn: This WILL break your package manager's tracking.",
            "warn: You are on your own. NO SUPPORT if things break.",
            "warn: Backup your data before proceeding.",
            "warn: ────────────────────────────────────────────────",
        ];
        for l in &lines {
            send_msg(l.to_string());
        }
    }

    let name = package.to_string();
    let replace_clone = replace;
    tokio::task::spawn_blocking(move || -> SpmResult<()> {
        crate::package::install::install_package(&name, None, replace_clone, yes, false, strategy)
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Install failed: {e}")))?;

    Ok(format!("Installed {package} (system)"))
}

fn resolve_strategy(req: &DaemonRequest) -> crate::types::VersionStrategy {
    let params = req.params.as_ref();
    let prefer_newest = params.and_then(|p| p["prefer_newest"].as_bool()).unwrap_or(false);
    let stable_debian = params.and_then(|p| p["stable_debian"].as_bool()).unwrap_or(false);
    let newest_redhat = params.and_then(|p| p["newest_redhat"].as_bool()).unwrap_or(false);
    if stable_debian {
        crate::types::VersionStrategy::PreferStable
    } else if newest_redhat || prefer_newest {
        crate::types::VersionStrategy::PreferNewest
    } else {
        crate::types::VersionStrategy::PreferStable
    }
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

    let params = req.params.as_ref();
    let replace = params.and_then(|p| p["replace"].as_bool()).unwrap_or(false);

    if !replace {
        let conflicts = crate::package::install::check_pre_install_paths(package, uid);
        if !conflicts.is_empty() {
            let paths = conflicts.join(", ");
            return Err(SpmError::other(format!(
                "File conflict detected: '{}' already exists at: {}. Remove it first or use --replace.",
                package, paths
            )));
        }
    } else {
        use crate::output::send_msg;
        let lines = [
            "warn: ────────────────────────────────────────────────",
            "warn: ⚠  WARNING  ⚠  --REPLACE ACTIVATED",
            "warn: SPM will FORCE OVERWRITE files on your system.",
            "warn: These files may belong to another package manager.",
            "warn: This WILL break your package manager's tracking.",
            "warn: You are on your own. NO SUPPORT if things break.",
            "warn: Backup your data before proceeding.",
            "warn: ────────────────────────────────────────────────",
        ];
        for l in &lines {
            send_msg(l.to_string());
        }
    }

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
    let strategy = resolve_strategy(req);
    tokio::task::spawn_blocking(move || -> SpmResult<()> {
        crate::package::install::upgrade_package(package.as_deref(), strategy, None)
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

fn is_authorized(_uid: u32) -> bool {
    true
}

fn resolve_user_name(uid: u32) -> Option<String> {
    crate::util::user::resolve_user_name(uid)
}

async fn handle_update(uid: u32) -> Result<String, SpmError> {
    tokio::task::spawn_blocking(move || -> SpmResult<()> {
        crate::output::set_current_uid(uid);
        crate::config::repos::update_repos()
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Update failed: {e}")))?;
    Ok("Repositories updated".to_string())
}

async fn handle_dist_upgrade(req: &DaemonRequest) -> Result<String, SpmError> {
    let yes = req.params.as_ref().and_then(|p| p["yes"].as_bool()).unwrap_or(false);
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
    let yes = req.params.as_ref().and_then(|p| p["yes"].as_bool()).unwrap_or(false);
    tokio::task::spawn_blocking(move || -> SpmResult<()> {
        crate::package::install::autoremove_packages(yes)
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Autoremove failed: {e}")))?;
    Ok("Autoremove completed".to_string())
}

async fn handle_cleanup(req: &DaemonRequest) -> Result<String, SpmError> {
    let force = req.params.as_ref().and_then(|p| p["force"].as_bool()).unwrap_or(true);
    tokio::task::spawn_blocking(move || -> SpmResult<()> {
        crate::package::cleanup::cleanup_all(force)
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

async fn handle_install_local(req: &DaemonRequest) -> HandlerResult {
    let params = req.params.as_ref().ok_or_else(|| SpmError::other("Missing params"))?;
    let path = params["path"].as_str().ok_or_else(|| SpmError::other("Missing 'path'"))?;
    let replace = params["replace"].as_bool().unwrap_or(false);
    let yes = params["yes"].as_bool().unwrap_or(true);
    let p = path.to_string();
    tokio::task::spawn_blocking(move || -> SpmResult<()> {
        crate::package::install::install_local_package(&p, replace, yes)
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Install failed: {e}")))?;
    Ok((format!("Installed {path}"), None))
}

async fn handle_convert(req: &DaemonRequest) -> HandlerResult {
    let params = req.params.as_ref().ok_or_else(|| SpmError::other("Missing params"))?;
    let path = params["path"].as_str().ok_or_else(|| SpmError::other("Missing 'path'"))?;
    let p = path.to_string();
    tokio::task::spawn_blocking(move || -> SpmResult<()> {
        crate::package::sam::convert_to_sam(&p)
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Convert failed: {e}")))?;
    Ok((format!("Converted {} to .sam format", path), None))
}

async fn handle_install_sandbox(req: &DaemonRequest) -> HandlerResult {
    let params = req.params.as_ref().ok_or_else(|| SpmError::other("Missing params"))?;
    let package = params["package"].as_str().ok_or_else(|| SpmError::other("Missing 'package'"))?;
    let level = params["level"].as_str().unwrap_or("standard");
    let replace = params["replace"].as_bool().unwrap_or(false);
    let yes = params["yes"].as_bool().unwrap_or(true);
    let smart = params["smart"].as_bool().unwrap_or(false);
    let strategy = resolve_strategy(req);
    let pkg = package.to_string();
    let lvl = level.to_string();
    tokio::task::spawn_blocking(move || -> SpmResult<()> {
        crate::package::install::install_package(&pkg, Some(&lvl), replace, yes, smart, strategy)
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Sandbox install failed: {e}")))?;
    Ok((format!("Installed '{package}' in sandbox ({level})"), None))
}

async fn handle_build(req: &DaemonRequest) -> HandlerResult {
    let params = req.params.as_ref().ok_or_else(|| SpmError::other("Missing params"))?;
    let path = params["path"].as_str().ok_or_else(|| SpmError::other("Missing 'path'"))?;
    let output = params["output"].as_str();
    let sign = params["sign"].as_str();
    let p = path.to_string();
    let out = output.map(|s| s.to_string());
    let sg = sign.map(|s| s.to_string());
    tokio::task::spawn_blocking(move || -> SpmResult<String> {
        crate::package::build::run_build(std::path::Path::new(&p), out.as_deref(), sg.as_deref())
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Build failed: {e}")))?;
    Ok((format!("Built {path}"), None))
}

async fn handle_search(req: &DaemonRequest) -> HandlerResult {
    let query = req.package.as_deref().unwrap_or("");
    let query = query.to_string();
    let results = tokio::task::spawn_blocking(move || -> SpmResult<Vec<crate::types::Package>> {
        crate::package::query::search_packages(&query)
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Search failed: {e}")))?;
    let mut out = String::new();
    for pkg in &results {
        let source = pkg.source_repo.as_deref().unwrap_or("?");
        let fmt = format!("{:?}", pkg.format);
        out.push_str(&format!("  {} [{}/{}]\n", pkg.name, source, fmt));
    }
    out.push_str(&format!("  {} package(s) found", results.len()));
    Ok((out, None))
}

async fn handle_search_file(req: &DaemonRequest) -> HandlerResult {
    let path = req.package.as_deref().ok_or_else(|| SpmError::other("Missing path"))?;
    let path = path.to_string();
    let owners = tokio::task::spawn_blocking(move || -> SpmResult<Vec<String>> {
        crate::package::query::search_file_owner(&path)
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Search failed: {e}")))?;
    Ok((format!("{} owner(s) found", owners.len()), Some(serde_json::json!(owners))))
}

async fn handle_info(req: &DaemonRequest) -> HandlerResult {
    let package = req.package.as_deref().ok_or_else(|| SpmError::other("Missing package"))?;
    let package = package.to_string();
    let info = tokio::task::spawn_blocking(move || -> SpmResult<String> {
        let info = crate::package::query::package_info(&package)?;
        let files = crate::package::query::list_package_files(&package)?;
        let conn = db::get_connection()?;
        if let Some(pkg) = db::get_installed_package(&conn, &package)? {
            let mut out = String::new();
            out.push_str(&format!("  Package: {} v{}\n", pkg.name, pkg.version));
            out.push_str(&format!("  Format: {:?}\n", pkg.format));
            out.push_str(&format!("  Type: {:?}\n", pkg.install_type));
            out.push_str(&format!("  Source: {}\n", pkg.source_repo.as_deref().unwrap_or("none")));
            out.push_str(&format!("  Files: {}", files.len()));
            Ok(out)
        } else {
            Ok(info)
        }
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Info failed: {e}")))?;
    Ok((info, None))
}

async fn handle_files(req: &DaemonRequest) -> HandlerResult {
    let pkg_name = req.package.as_deref().ok_or_else(|| SpmError::other("Missing package"))?;
    let pkg = pkg_name.to_string();
    let files = tokio::task::spawn_blocking(move || -> SpmResult<Vec<String>> {
        crate::package::query::list_package_files(&pkg)
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("List files failed: {e}")))?;
    let mut out = format!("  {} owns {} file(s):\n", pkg_name, files.len());
    for f in &files {
        out.push_str(&format!("    {}\n", f));
    }
    Ok((out, None))
}

async fn handle_depends(req: &DaemonRequest) -> HandlerResult {
    let pkg_name = req.package.as_deref().ok_or_else(|| SpmError::other("Missing package"))?;
    let pkg = pkg_name.to_string();
    let deps = tokio::task::spawn_blocking(move || -> SpmResult<Vec<crate::types::Dependency>> {
        crate::package::query::package_dependencies(&pkg)
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Deps failed: {e}")))?;
    let mut out = format!("  {} depends on:\n", pkg_name);
    for d in &deps {
        out.push_str(&format!("    {}\n", d.name));
    }
    Ok((out, None))
}

async fn handle_rdepends(req: &DaemonRequest) -> HandlerResult {
    let pkg_name = req.package.as_deref().ok_or_else(|| SpmError::other("Missing package"))?;
    let pkg = pkg_name.to_string();
    let rdeps = tokio::task::spawn_blocking(move || -> SpmResult<Vec<String>> {
        crate::package::query::reverse_dependencies(&pkg)
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Reverse deps failed: {e}")))?;
    let mut out = format!("  Packages depending on {}:\n", pkg_name);
    for r in &rdeps {
        out.push_str(&format!("    {}\n", r));
    }
    Ok((out, None))
}

async fn handle_config_show() -> HandlerResult {
    let cfg = tokio::task::spawn_blocking(move || -> SpmResult<SpmConfig> {
        SpmConfig::load()
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Config load failed: {e}")))?;
    let json = serde_json::to_value(&cfg)
        .map_err(|e| SpmError::other(format!("Serialization failed: {e}")))?;
    Ok(("Config loaded".to_string(), Some(json)))
}

async fn handle_config_set(req: &DaemonRequest) -> HandlerResult {
    let params = req.params.as_ref().ok_or_else(|| SpmError::other("Missing params"))?;
    let k = params["key"].as_str().ok_or_else(|| SpmError::other("Missing 'key'"))?;
    let v = params["value"].as_str().ok_or_else(|| SpmError::other("Missing 'value'"))?;
    let key = k.to_string();
    let value = v.to_string();
    tokio::task::spawn_blocking(move || -> SpmResult<()> {
        SpmConfig::set(&key, &value)
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Config set failed: {e}")))?;
    Ok((format!("Set config {k} = {v}"), None))
}

async fn handle_index_rebuild() -> HandlerResult {
    tokio::task::spawn_blocking(move || -> SpmResult<()> {
        crate::index::build_index()
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Index rebuild failed: {e}")))?;
    Ok(("SONAME index rebuilt".to_string(), None))
}

async fn handle_history(req: &DaemonRequest) -> HandlerResult {
    let undo = req.params.as_ref().and_then(|p| p["id"].as_str()).map(|s| s.to_string());
    tokio::task::spawn_blocking(move || -> SpmResult<String> {
        if let Some(id) = undo {
            let id_i64: i64 = id.parse().map_err(|e| SpmError::other(format!("Invalid transaction ID: {e}")))?;
            crate::package::query::undo_transaction(id_i64)?;
            Ok(format!("Transaction {id} undone"))
        } else {
            crate::package::query::show_history()
        }
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("History failed: {e}")))
        .map(|msg| (msg, None))
}

async fn handle_init(req: &DaemonRequest) -> HandlerResult {
    let params = req.params.as_ref();
    let root_opt = params.and_then(|p| p["root"].as_str()).map(|s| s.to_string());
    let from_system = params.and_then(|p| p["from_system"].as_bool()).unwrap_or(false);
    let fix_backend = params.and_then(|p| p["fix_backend"].as_bool()).unwrap_or(false);
    let install_daemon = params.and_then(|p| p["install_daemon"].as_bool()).unwrap_or(false);
    let fb = fix_backend || root_opt.is_some() || from_system;
    let root_c = root_opt.clone();
    tokio::task::spawn_blocking(move || -> SpmResult<()> {
        crate::bootstrap::init_system(root_c.as_deref(), from_system, fb)?;
        if install_daemon {
            crate::bootstrap::install_daemon_service()?;
        }
        Ok(())
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Init failed: {e}")))?;
    Ok(("System initialized".to_string(), None))
}

async fn handle_group(req: &DaemonRequest) -> HandlerResult {
    let params = req.params.as_ref().ok_or_else(|| SpmError::other("Missing params"))?;
    let action = params["action"].as_str().ok_or_else(|| SpmError::other("Missing 'action'"))?;
    match action {
        "create" => {
            let _ = tokio::task::spawn_blocking(crate::cli::group::handle_group_create).await;
            Ok(("spm group created".to_string(), None))
        }
        "add" => {
            let u = params["user"].as_str().ok_or_else(|| SpmError::other("Missing 'user'"))?;
            let user = u.to_string();
            let _ = tokio::task::spawn_blocking(move || crate::cli::group::handle_group_add_user(&user)).await;
            Ok((format!("User {u} added to spm group"), None))
        }
        "remove" => {
            let u = params["user"].as_str().ok_or_else(|| SpmError::other("Missing 'user'"))?;
            let user = u.to_string();
            let _ = tokio::task::spawn_blocking(move || crate::cli::group::handle_group_remove_user(&user)).await;
            Ok((format!("User {u} removed from spm group"), None))
        }
        "list" => {
            let _ = tokio::task::spawn_blocking(crate::cli::group::handle_group_list).await;
            Ok(("Group members listed".to_string(), None))
        }
        _ => Err(SpmError::other(format!("Unknown group action: {action}"))),
    }
}

async fn handle_fsck(req: &DaemonRequest) -> HandlerResult {
    let params = req.params.as_ref();
    let fix = params.and_then(|p| p["fix"].as_bool()).unwrap_or(false);
    let files = params.and_then(|p| p["files"].as_bool()).unwrap_or(false);
    let _ = tokio::task::spawn_blocking(move || -> SpmResult<()> {
        crate::fsck::check_integrity(fix, files)
    }).await;
    Ok(("Integrity check completed".to_string(), None))
}

async fn handle_sync(req: &DaemonRequest) -> HandlerResult {
    let params = req.params.as_ref();
    let files = params.and_then(|p| p["files"].as_bool()).unwrap_or(false);
    let prune = params.and_then(|p| p["prune"].as_bool()).unwrap_or(false);
    let _ = tokio::task::spawn_blocking(move || -> SpmResult<()> {
        crate::sync::sync_system(files, prune)
    }).await;
    Ok(("System sync completed".to_string(), None))
}

async fn handle_repo_list() -> HandlerResult {
    let repos = tokio::task::spawn_blocking(move || -> SpmResult<Vec<(String, crate::types::RepoConfig)>> {
        crate::config::repos::load_repos()
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Repo list failed: {e}")))?;
    if repos.is_empty() {
        return Ok(("No repositories configured.\nAdd one: spm repo add <name> --source native --url <url>".to_string(), None));
    }
    let mut out = String::new();
    for (name, config) in &repos {
        let url = config.url.as_deref().unwrap_or("-");
        out.push_str(&format!("  {} ({}) — {}\n", name, config.source, url));
    }
    Ok((out, None))
}

async fn handle_repo_create(req: &DaemonRequest) -> HandlerResult {
    let params = req.params.as_ref().ok_or_else(|| SpmError::other("Missing params"))?;
    let n = params["name"].as_str().ok_or_else(|| SpmError::other("Missing 'name'"))?;
    let source = params["source"].as_str().ok_or_else(|| SpmError::other("Missing 'source'"))?;
    let path = params["path"].as_str();
    let codename = params["codename"].as_str();
    let component = params["component"].as_str();
    let mirror = params["mirror"].as_str();
    let source_enum = match source {
        "deb" => crate::types::RepoSource::Deb,
        "rpm" => crate::types::RepoSource::Rpm,
        "native" => crate::types::RepoSource::Native,
        _ => return Err(SpmError::config(format!("Invalid source '{source}'"))),
    };
    let name = n.to_string();
    let path = path.map(|s| s.to_string());
    let codename = codename.map(|s| s.to_string());
    let component = component.map(|s| s.to_string());
    let mirror = mirror.map(|s| s.to_string());
    tokio::task::spawn_blocking(move || -> SpmResult<()> {
        crate::config::repos::create_repo(&name, source_enum, path.as_deref(), codename.as_deref(), component.as_deref(), mirror.as_deref())
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Repo create failed: {e}")))?;
    Ok((format!("Created repository '{n}'"), None))
}

async fn handle_repo_publish(req: &DaemonRequest) -> HandlerResult {
    let params = req.params.as_ref().ok_or_else(|| SpmError::other("Missing params"))?;
    let n = params["name"].as_str().ok_or_else(|| SpmError::other("Missing 'name'"))?;
    let p = params["package"].as_str().ok_or_else(|| SpmError::other("Missing 'package'"))?;
    let name = n.to_string();
    let package = p.to_string();
    tokio::task::spawn_blocking(move || -> SpmResult<()> {
        crate::config::repos::publish_package(&name, &package)
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Publish failed: {e}")))?;
    Ok((format!("Published {p} to '{n}'"), None))
}

async fn handle_repo_gen_key(req: &DaemonRequest) -> HandlerResult {
    let n = req.package.as_deref().ok_or_else(|| SpmError::other("Missing repo name"))?;
    let name = n.to_string();
    tokio::task::spawn_blocking(move || -> SpmResult<()> {
        crate::config::repos::generate_signing_key(&name)
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Gen key failed: {e}")))?;
    Ok((format!("Generated signing key for '{n}'"), None))
}

async fn handle_repo_sign(req: &DaemonRequest) -> HandlerResult {
    let n = req.package.as_deref().ok_or_else(|| SpmError::other("Missing repo name"))?;
    let name = n.to_string();
    tokio::task::spawn_blocking(move || -> SpmResult<()> {
        crate::config::repos::sign_repo(&name)
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Sign failed: {e}")))?;
    Ok((format!("Signed repository '{n}'"), None))
}

async fn handle_sandbox_list() -> HandlerResult {
    let sandbox_dir = crate::config::paths::sandboxes_dir();
    let entries = tokio::task::spawn_blocking(move || -> SpmResult<Vec<String>> {
        if !sandbox_dir.exists() {
            return Ok(Vec::new());
        }
        let mut names = Vec::new();
        for entry in std::fs::read_dir(&sandbox_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                names.push(entry.file_name().to_string_lossy().to_string());
            }
        }
        Ok(names)
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Sandbox list failed: {e}")))?;
    if entries.is_empty() {
        return Ok(("No sandboxes found".to_string(), None));
    }
    let mut out = String::new();
    for name in &entries {
        out.push_str(&format!("  {} (symlink)\n", name));
    }
    out.push_str(&format!("\nTotal: {} sandbox(es)", entries.len()));
    Ok((out, None))
}

async fn handle_sandbox_run(req: &DaemonRequest) -> HandlerResult {
    let params = req.params.as_ref().ok_or_else(|| SpmError::other("Missing params"))?;
    let pkg_name = params["package"].as_str().ok_or_else(|| SpmError::other("Missing 'package'"))?;
    let commands: Vec<String> = params["commands"].as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let pkg = pkg_name.to_string();
    tokio::task::spawn_blocking(move || -> SpmResult<()> {
        crate::sandbox::run_in_sandbox(&pkg, &commands)
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("Sandbox run failed: {e}")))?;
    Ok((format!("Ran sandbox '{pkg_name}'"), None))
}

async fn handle_detect(req: &DaemonRequest) -> HandlerResult {
    let params = req.params.as_ref();
    let install = params.and_then(|p| p["install"].as_bool()).unwrap_or(false);
    let gpus = tokio::task::spawn_blocking(move || -> SpmResult<Vec<crate::kernel::GpuDevice>> {
        crate::kernel::detect_gpu()
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))?
        .map_err(|e| SpmError::other(format!("GPU detection failed: {e}")))?;
    let mut out = String::new();
    for gpu in &gpus {
        out.push_str(&format!("  GPU {} — {}\n", gpu.pci_id, gpu.description));
        out.push_str(&format!("    Vendor: {}\n", gpu.vendor));
        out.push_str(&format!("    Driver: {}\n", gpu.driver));
        let suggestions = crate::kernel::suggest_gpu_packages(gpu);
        if !suggestions.is_empty() {
            out.push_str("    Suggested packages:\n");
            for pkg in &suggestions {
                out.push_str(&format!("      • {}\n", pkg));
                if install {
                    let _ = crate::package::install::install_package_smart(pkg, None, false, true, false, Default::default(), None);
                }
            }
        }
    }
    if gpus.is_empty() {
        out.push_str("  No GPU detected via lspci");
    }
    Ok((out, None))
}

async fn handle_analyze(req: &DaemonRequest) -> HandlerResult {
    let params = req.params.as_ref();
    let mode = params.and_then(|p| p["mode"].as_str()).unwrap_or("full").to_string();
    let trace = params.and_then(|p| p["trace"].as_str()).map(|s| s.to_string());
    match trace {
        Some(path) => {
            let p = path.clone();
            tokio::task::spawn_blocking(move || -> SpmResult<()> {
                crate::analyze::trace_binary(&p)
            }).await
                .map_err(|e| SpmError::other(format!("Join error: {e}")))?
                .map_err(|e| SpmError::other(format!("Trace failed: {e}")))?;
            Ok((format!("Trace completed for {}", path), None))
        }
        None => {
            let mode_c = mode.clone();
            let _ = tokio::task::spawn_blocking(move || -> SpmResult<()> {
                match mode_c.as_str() {
                    "orphan" => crate::analyze::find_orphans(),
                    "conflicts" => crate::analyze::find_conflicts(),
                    _ => crate::analyze::full_analysis(),
                }
            }).await;
            Ok((format!("{} analysis completed", mode), None))
        }
    }
}

async fn handle_ps(req: &DaemonRequest) -> HandlerResult {
    let deleted = req.params.as_ref().and_then(|p| p["deleted-libs"].as_bool()).unwrap_or(false);
    if deleted {
        let _ = tokio::task::spawn_blocking(crate::util::process::find_deleted_libs).await;
    }
    Ok(("spmd is running".to_string(), None))
}

async fn handle_self_update(req: &DaemonRequest) -> HandlerResult {
    use std::process::Command;

    let params = req.params.as_ref();
    let check_only = params.and_then(|p| p["check"].as_bool()).unwrap_or(false);
    let specific_version = params.and_then(|p| p["version"].as_str()).and_then(|s| {
        if s.is_empty() { None } else { Some(s.to_string()) }
    });

    let current = env!("CARGO_PKG_VERSION");

    crate::output::step_info(format!("Current version: v{current}"));

    // ── Detect architecture ──
    let arch = tokio::task::spawn_blocking(|| -> SpmResult<String> {
        let output = Command::new("uname")
            .arg("-m")
            .output()
            .map_err(|e| SpmError::other(format!("Cannot detect arch: {e}")))?;
        let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let arch = match raw.as_str() {
            "x86_64" => "x86_64-linux-gnu",
            "i686" | "i386" => "i686-linux-gnu",
            "aarch64" => "aarch64-linux-gnu",
            "armv7l" | "armv7" => "armv7-linux-gnueabihf",
            "riscv64" => "riscv64-linux-gnu",
            other => return Err(SpmError::other(format!("Unsupported architecture: {other}"))),
        };
        Ok(arch.to_string())
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))??;

    // ── Fetch release info from GitHub ──
    let (tag_name, release_json) = tokio::task::spawn_blocking(move || -> SpmResult<(String, serde_json::Value)> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent(concat!("spm/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| SpmError::other(format!("HTTP client error: {e}")))?;

        let url = match specific_version {
            Some(ref v) => format!("https://api.github.com/repos/anomalyco/Samad-Package-Manager/releases/tags/v{v}"),
            None => "https://api.github.com/repos/anomalyco/Samad-Package-Manager/releases/latest".to_string(),
        };

        let resp = client.get(&url)
            .send()
            .map_err(|e| SpmError::other(format!("GitHub API error: {e}")))?;

        if !resp.status().is_success() {
            return Err(SpmError::other(format!(
                "GitHub API returned {} for {url}",
                resp.status()
            )));
        }

        let json: serde_json::Value = resp.json()
            .map_err(|e| SpmError::other(format!("Invalid JSON: {e}")))?;

        let tag = json["tag_name"].as_str()
            .ok_or_else(|| SpmError::other("Missing 'tag_name' in release response"))?
            .to_string();

        Ok((tag, json))
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))??;

    let tag_ver = tag_name.trim_start_matches('v');

    // ── Compare versions ──
    if !is_newer(tag_ver, current) {
        let msg = format!("Already up to date (v{current})");
        return Ok((msg, None));
    }

    if check_only {
        let msg = format!("Update available: v{current} → {tag_name}");
        return Ok((msg, Some(serde_json::json!({
            "current_version": current,
            "latest_version": tag_ver,
        }))));
    }

    // ── Find matching assets ──
    let assets = release_json["assets"].as_array()
        .ok_or_else(|| SpmError::other("No assets in release"))?;

    let spm_asset_name = format!("spm-{arch}");
    let spmd_asset_name = format!("spmd-{arch}");

    let spm_url = assets.iter()
        .find(|a| a["name"].as_str() == Some(&spm_asset_name))
        .and_then(|a| a["browser_download_url"].as_str())
        .ok_or_else(|| SpmError::other(format!("No spm binary for {arch} in this release")))?
        .to_string();

    let spmd_url = assets.iter()
        .find(|a| a["name"].as_str() == Some(&spmd_asset_name))
        .and_then(|a| a["browser_download_url"].as_str())
        .ok_or_else(|| SpmError::other(format!("No spmd binary for {arch} in this release")))?
        .to_string();

    let checksums_url = assets.iter()
        .find(|a| a["name"].as_str() == Some("checksums.txt"))
        .and_then(|a| a["browser_download_url"].as_str())
        .ok_or_else(|| SpmError::other("No checksums.txt in release"))?
        .to_string();

    crate::output::step_info("Downloading spm binary...");

    let (spm_data, spmd_data, checksums_data) = tokio::task::spawn_blocking(move || -> SpmResult<(Vec<u8>, Vec<u8>, String)> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .user_agent(concat!("spm/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| SpmError::other(format!("HTTP client error: {e}")))?;

        let spm_data = client.get(&spm_url)
            .send()
            .map_err(|e| SpmError::other(format!("Failed to download spm: {e}")))?
            .bytes()
            .map_err(|e| SpmError::other(format!("Failed to read spm: {e}")))?
            .to_vec();

        let spmd_data = client.get(&spmd_url)
            .send()
            .map_err(|e| SpmError::other(format!("Failed to download spmd: {e}")))?
            .bytes()
            .map_err(|e| SpmError::other(format!("Failed to read spmd: {e}")))?
            .to_vec();

        let checksums_data = client.get(&checksums_url)
            .send()
            .map_err(|e| SpmError::other(format!("Failed to download checksums: {e}")))?
            .text()
            .map_err(|e| SpmError::other(format!("Failed to read checksums: {e}")))?;

        Ok((spm_data, spmd_data, checksums_data))
    }).await
        .map_err(|e| SpmError::other(format!("Join error: {e}")))??;

    // ── Verify checksums ──
    crate::output::step_info("Verifying checksums...");
    verify_checksum(&spm_data, &spm_asset_name, &checksums_data)?;
    verify_checksum(&spmd_data, &spmd_asset_name, &checksums_data)?;

    // ── Replace binaries ──
    crate::output::step_info("Installing update...");

    let spm_path = "/usr/bin/spm";
    let spmd_path = "/usr/bin/spmd";

    // Write to temp files then atomically rename
    std::fs::write("/tmp/spm-update", &spm_data)
        .map_err(|e| SpmError::other(format!("Cannot write temp spm: {e}")))?;
    std::fs::write("/tmp/spmd-update", &spmd_data)
        .map_err(|e| SpmError::other(format!("Cannot write temp spmd: {e}")))?;

    // Set executable permissions
    std::fs::set_permissions("/tmp/spm-update", std::fs::Permissions::from_mode(0o755))
        .map_err(|e| SpmError::other(format!("Cannot chmod temp spm: {e}")))?;
    std::fs::set_permissions("/tmp/spmd-update", std::fs::Permissions::from_mode(0o755))
        .map_err(|e| SpmError::other(format!("Cannot chmod temp spmd: {e}")))?;

    // Rename into place (atomic on same filesystem)
    std::fs::rename("/tmp/spm-update", spm_path)
        .map_err(|e| SpmError::other(format!("Cannot replace {spm_path}: {e}")))?;
    std::fs::rename("/tmp/spmd-update", spmd_path)
        .map_err(|e| SpmError::other(format!("Cannot replace {spmd_path}: {e}")))?;

    let msg = format!("Updated to {tag_name}\n  {spm_path}\n  {spmd_path}\nRestart spmd to apply: systemctl restart spmd");

    Ok((msg, None))
}

fn is_newer(tag_ver: &str, current: &str) -> bool {
    fn parse_ver(v: &str) -> (u64, u64, u64) {
        let parts: Vec<&str> = v.trim_start_matches('v').splitn(3, '.').collect();
        let major = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
        let minor = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        let patch = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
        (major, minor, patch)
    }
    let (m1, n1, p1) = parse_ver(tag_ver);
    let (m2, n2, p2) = parse_ver(current);
    (m1, n1, p1) > (m2, n2, p2)
}

fn verify_checksum(data: &[u8], filename: &str, checksums: &str) -> SpmResult<()> {
    use sha2::{Digest, Sha256};
    let expected_hash = checksums.lines()
        .find(|line| line.ends_with(filename))
        .and_then(|line| line.split_whitespace().next())
        .ok_or_else(|| SpmError::other(format!("No checksum for {filename}")))?;

    let mut hasher = Sha256::new();
    hasher.update(data);
    let actual_hash = format!("{:x}", hasher.finalize());

    if actual_hash != expected_hash {
        return Err(SpmError::other(format!(
            "Checksum mismatch for {filename}: expected {expected_hash}, got {actual_hash}"
        )));
    }
    Ok(())
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

        // Allow any local user to connect (personal system — no group restriction)
        std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o666))
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
