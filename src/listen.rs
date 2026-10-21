//! PRD-mcphost-subscriptions-listen: the per-tenant hub that feeds open
//! `subscriptions/listen` streams.
//!
//! rmcp builds a new `McpHostHandler` for every POST, and on the stateless
//! transport a request's peer closes when its response ends, so a standing
//! listener can only live in shared state: [`ListenHub`] is a field of
//! `AppState`. A listen registers one entry per open stream under its tenant
//! id; a committed change to that tenant's lists is sent to every entry of
//! the tenant and to no other tenant's.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::{broadcast, watch};

/// Messages a lagging stream may drop: a list change is idempotent (the client
/// re-reads the list), so a stream that falls this far behind gets one
/// notification and not a backlog.
const CHANNEL_CAPACITY: usize = 16;

/// Which list a hub message says changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListChange {
    Tools,
    Prompts,
}

struct Entry {
    tx: broadcast::Sender<ListChange>,
    open: usize,
}

struct Inner {
    tenants: Mutex<HashMap<i64, Entry>>,
    shutdown: watch::Sender<bool>,
}

/// The per-tenant registry of open listen streams. Cheap to clone: every clone
/// shares one map.
#[derive(Clone)]
pub struct ListenHub(Arc<Inner>);

impl Default for ListenHub {
    fn default() -> Self {
        Self(Arc::new(Inner { tenants: Mutex::new(HashMap::new()), shutdown: watch::channel(false).0 }))
    }
}

impl ListenHub {
    fn lock(&self) -> MutexGuard<'_, HashMap<i64, Entry>> {
        self.0.tenants.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Register one open stream for `tenant_id`. The returned guard removes the
    /// registration when it drops, whichever way the stream ends.
    pub fn open(&self, tenant_id: i64) -> ListenGuard {
        let mut tenants = self.lock();
        let entry = tenants.entry(tenant_id).or_insert_with(|| Entry {
            tx: broadcast::channel(CHANNEL_CAPACITY).0,
            open: 0,
        });
        entry.open += 1;
        ListenGuard { hub: self.clone(), tenant_id, rx: entry.tx.subscribe() }
    }

    /// Tell every open stream of `tenant_id` that `change` happened. A tenant
    /// with no open stream costs one map lookup.
    pub fn notify(&self, tenant_id: i64, change: ListChange) {
        if let Some(entry) = self.lock().get(&tenant_id) {
            let _ = entry.tx.send(change);
        }
    }

    /// Open streams `tenant_id` holds right now.
    pub fn open_for(&self, tenant_id: i64) -> usize {
        self.lock().get(&tenant_id).map_or(0, |e| e.open)
    }

    /// Tenants the hub holds an entry for. 0 once every stream has closed.
    pub fn entries(&self) -> usize {
        self.lock().len()
    }

    /// Ask every open stream to end with the completion result (server shutdown).
    pub fn shutdown(&self) {
        let _ = self.0.shutdown.send(true);
    }

    fn shutdown_signal(&self) -> watch::Receiver<bool> {
        self.0.shutdown.subscribe()
    }
}

/// One open stream's registration and its feed.
pub struct ListenGuard {
    hub: ListenHub,
    tenant_id: i64,
    rx: broadcast::Receiver<ListChange>,
}

impl ListenGuard {
    /// The next change for this stream, or `None` once the hub is shutting down.
    /// A stream that lagged past the channel capacity gets `Tools` and then
    /// `Prompts` once each: it cannot tell which messages it lost.
    pub async fn next(&mut self, pending: &mut Vec<ListChange>) -> Option<ListChange> {
        if let Some(change) = pending.pop() {
            return Some(change);
        }
        let mut shutdown = self.hub.shutdown_signal();
        tokio::select! {
            _ = shutdown.wait_for(|down| *down) => None,
            msg = self.rx.recv() => match msg {
                Ok(change) => Some(change),
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    pending.push(ListChange::Prompts);
                    Some(ListChange::Tools)
                }
                Err(broadcast::error::RecvError::Closed) => None,
            },
        }
    }
}

impl Drop for ListenGuard {
    fn drop(&mut self) {
        let mut tenants = self.hub.lock();
        if let Some(entry) = tenants.get_mut(&self.tenant_id) {
            entry.open = entry.open.saturating_sub(1);
            if entry.open == 0 {
                tenants.remove(&self.tenant_id);
            }
        }
    }
}
