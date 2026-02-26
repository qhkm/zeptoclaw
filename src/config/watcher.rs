//! File-mtime polling watcher for hot-reloading config.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};

use crate::config::Config;

/// Polling-based config watcher.
pub struct ConfigWatcher {
    path: PathBuf,
    poll_interval: Duration,
    last_mtime: Option<SystemTime>,
}

impl ConfigWatcher {
    pub fn new(path: PathBuf, poll_interval: Duration) -> Self {
        Self {
            path,
            poll_interval,
            last_mtime: None,
        }
    }

    pub fn default_path(poll_interval: Duration) -> Self {
        Self::new(Config::path(), poll_interval)
    }

    pub async fn watch(
        mut self,
        tx: mpsc::UnboundedSender<Config>,
        mut shutdown_rx: watch::Receiver<bool>,
    ) {
        self.last_mtime = read_mtime(&self.path);
        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("Config watcher shutting down");
                        return;
                    }
                }
                _ = tokio::time::sleep(self.poll_interval) => {}
            }

            if *shutdown_rx.borrow() {
                return;
            }

            let current = read_mtime(&self.path);
            let changed = match (self.last_mtime, current) {
                (Some(prev), Some(next)) => next != prev,
                (None, Some(_)) => true,
                _ => false,
            };
            if !changed {
                continue;
            }

            self.last_mtime = current;
            match Config::load_from_path(&self.path) {
                Ok(config) => {
                    debug!(path = %self.path.display(), "Config file changed, reloading");
                    if tx.send(config).is_err() {
                        warn!("Config watcher receiver dropped, stopping watcher");
                        return;
                    }
                }
                Err(err) => {
                    warn!(
                        path = %self.path.display(),
                        error = %err,
                        "Config reload rejected; keeping running configuration"
                    );
                }
            }
        }
    }
}

fn read_mtime(path: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn watcher_emits_on_change() {
        let tmp = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.json");
        std::fs::write(&cfg_path, "{}").unwrap();

        let (tx, mut rx) = mpsc::unbounded_channel();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let watcher = ConfigWatcher::new(cfg_path.clone(), Duration::from_millis(25));
        let handle = tokio::spawn(watcher.watch(tx, shutdown_rx));

        tokio::time::sleep(Duration::from_millis(40)).await;
        std::fs::write(&cfg_path, r#"{"gateway":{"port":9898}}"#).unwrap();

        let loaded = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.gateway.port, 9898);

        let _ = shutdown_tx.send(true);
        let _ = handle.await;
    }
}
