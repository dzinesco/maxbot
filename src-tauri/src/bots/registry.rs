//! In-process registry of currently-running bot runs. The `run_id`
//! (a fresh UUID generated per `run_bot_once`) is mapped to the
//! `CancellationToken` the executor checks between iterations and
//! during tool calls. The `stop_bot_run` Tauri command looks up the
//! token by id and cancels it.
//!
//! Lifetime: a token lives in the map from the start of a run until
//! the executor removes it on completion (success, failure, or
//! cancellation). Stale entries (e.g. the executor panicked and never
//! cleaned up) are evicted on the next insert by capping the map.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub struct BotRunRegistry {
    active: Mutex<HashMap<String, CancellationToken>>,
}

impl BotRunRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an externally-created token. The caller can fire
    /// the token from anywhere (e.g. via `cancel.run_id` on the
    /// parent token) and the registry is just the lookup index for
    /// the UI. Used by the executor to surface its `cancel` parameter
    /// to the Stop button.
    pub async fn register(&self, run_id: String, token: CancellationToken) {
        let mut map = self.active.lock().await;
        if map.len() >= 64 {
            map.retain(|_, t| !t.is_cancelled());
        }
        map.insert(run_id, token);
    }

    /// Look up a token and cancel it. Returns true if a token was
    /// found (and cancelled), false if no run is active with that id.
    pub async fn cancel(&self, run_id: &str) -> bool {
        let map = self.active.lock().await;
        if let Some(token) = map.get(run_id) {
            token.cancel();
            true
        } else {
            false
        }
    }

    /// Remove a token from the registry. Called by the executor on
    /// completion (success, failure, or cancellation) so the map
    /// doesn't grow unbounded.
    pub async fn unregister(&self, run_id: &str) {
        self.active.lock().await.remove(run_id);
    }

    /// True iff a token with this id is currently registered (and
    /// therefore cancellable).
    pub async fn is_active(&self, run_id: &str) -> bool {
        self.active.lock().await.contains_key(run_id)
    }
}

/// Shared handle the Tauri command side keeps in AppState.
pub type SharedBotRunRegistry = Arc<BotRunRegistry>;
