use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use thiserror::Error;
use tokio::sync::mpsc;

use crate::LocalPaths;

/// Native filesystem notifications are latency hints only. The receiver tells callers whether
/// notify reported an error, in which case reconciliation should fall back to a full hash scan.
pub struct WorkspaceWatcher {
    _watcher: RecommendedWatcher,
    receiver: mpsc::Receiver<bool>,
    force_full_scan: Arc<AtomicBool>,
}

impl WorkspaceWatcher {
    pub fn start(paths: &LocalPaths) -> Result<Self, WorkspaceWatchError> {
        let (sender, receiver) = mpsc::channel(64);
        let force_full_scan = Arc::new(AtomicBool::new(false));
        let callback_force_full_scan = force_full_scan.clone();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let full_scan = event.is_err();
                if full_scan {
                    callback_force_full_scan.store(true, Ordering::Release);
                }
                if matches!(
                    sender.try_send(full_scan),
                    Err(mpsc::error::TrySendError::Full(_))
                ) {
                    // Event storms collapse into one authoritative verification scan instead of
                    // turning the watcher callback into an unbounded memory queue.
                    callback_force_full_scan.store(true, Ordering::Release);
                }
            })?;
        watcher.watch(&paths.skills, RecursiveMode::Recursive)?;
        watcher.watch(&paths.generations, RecursiveMode::Recursive)?;
        watcher.watch(&paths.derived, RecursiveMode::Recursive)?;
        Ok(Self {
            _watcher: watcher,
            receiver,
            force_full_scan,
        })
    }

    pub async fn changed(&mut self) -> bool {
        self.receiver.recv().await.unwrap_or(true)
            || self.force_full_scan.swap(false, Ordering::AcqRel)
    }

    pub fn drain_full_scan_hint(&mut self, mut full_scan: bool) -> bool {
        while let Ok(hint) = self.receiver.try_recv() {
            full_scan |= hint;
        }
        full_scan || self.force_full_scan.swap(false, Ordering::AcqRel)
    }
}

#[derive(Debug, Error)]
pub enum WorkspaceWatchError {
    #[error("workspace watcher failed: {0}")]
    Notify(#[from] notify::Error),
}
