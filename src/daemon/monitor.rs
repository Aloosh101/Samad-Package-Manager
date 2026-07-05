use std::path::Path;
use std::sync::mpsc;

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

use crate::error::{SpmError, SpmResult};

pub struct DaemonMonitor {
    watcher: Option<RecommendedWatcher>,
}

impl DaemonMonitor {
    pub fn new() -> Self {
        Self { watcher: None }
    }

    pub fn start_watching(&mut self) -> SpmResult<mpsc::Receiver<notify::Result<Event>>> {
        let (tx, rx) = mpsc::channel::<notify::Result<Event>>();

        let mut watcher = RecommendedWatcher::new(tx, Config::default())
            .map_err(|e| SpmError::other(format!("Failed to create file watcher: {e}")))?;

        let watch_paths = ["/usr/bin/", "/usr/lib/", "/etc/"];
        for path_str in &watch_paths {
            let p = Path::new(path_str);
            if p.exists() {
                watcher
                    .watch(p, RecursiveMode::Recursive)
                    .map_err(|e| {
                        SpmError::other(format!("Failed to watch {}: {e}", path_str))
                    })?;
                tracing::info!("Monitoring path: {path_str}");
            } else {
                tracing::warn!("Path does not exist, skipping: {path_str}");
            }
        }

        self.watcher = Some(watcher);
        Ok(rx)
    }

    pub fn stop_watching(&mut self) {
        self.watcher = None;
        tracing::info!("File system monitoring stopped");
    }

    pub fn is_watching(&self) -> bool {
        self.watcher.is_some()
    }
}
