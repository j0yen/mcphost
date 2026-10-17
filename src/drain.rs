//! PRD-mcphost-graceful-drain-contract: SIGTERM is a contract. On the signal
//! the listener stops accepting, calls already accepted run on until
//! `MCPHOST_DRAIN_S`, and each one still running at the bound is answered
//! with `interrupted` naming the `restart_id`. One process-global [`Drain`]
//! holds the counters the call path and the serve loop share.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;
use tokio::sync::watch;

/// R1: `MCPHOST_DRAIN_S` when unset.
pub const DEFAULT_DRAIN_S: u64 = 20;
/// R1: stays under `MAX_TIMEOUT_S = 60`.
pub const MAX_DRAIN_S: u64 = 55;

/// `MCPHOST_DRAIN_S` resolved: unset or unparsable reads as the default,
/// anything above [`MAX_DRAIN_S`] is capped.
pub fn bound_s_from_env() -> u64 {
    std::env::var("MCPHOST_DRAIN_S")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_DRAIN_S)
        .min(MAX_DRAIN_S)
}

/// What the signal captured.
#[derive(Debug, Clone)]
pub struct SignalInfo {
    pub restart_id: String,
    pub signal_at_unix: i64,
    pub in_flight_at_signal: u64,
    pub bound_s: u64,
    pub at: Instant,
}

pub struct Drain {
    in_flight: AtomicU64,
    interrupted: AtomicU64,
    signal: Mutex<Option<SignalInfo>>,
    draining_tx: watch::Sender<bool>,
    cancel_tx: watch::Sender<bool>,
}

/// The one [`Drain`] of this process.
pub fn global() -> &'static Drain {
    static DRAIN: OnceLock<Drain> = OnceLock::new();
    DRAIN.get_or_init(|| Drain {
        in_flight: AtomicU64::new(0),
        interrupted: AtomicU64::new(0),
        signal: Mutex::new(None),
        draining_tx: watch::channel(false).0,
        cancel_tx: watch::channel(false).0,
    })
}

/// Held for the life of one accepted call; dropping it leaves the count.
pub struct InFlight(&'static Drain);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Drain {
    pub fn enter(&'static self) -> InFlight {
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        InFlight(self)
    }

    pub fn in_flight(&self) -> u64 {
        self.in_flight.load(Ordering::SeqCst)
    }

    pub fn is_draining(&self) -> bool {
        *self.draining_tx.borrow()
    }

    pub fn info(&self) -> Option<SignalInfo> {
        self.signal.lock().ok().and_then(|g| g.clone())
    }

    pub fn restart_id(&self) -> Option<String> {
        self.info().map(|i| i.restart_id)
    }

    /// The signal arrived: snapshot the in-flight count, flip to draining.
    /// A second call is a no-op.
    pub fn begin(&self, bound_s: u64) -> SignalInfo {
        let mut guard = self.signal.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(info) = guard.as_ref() {
            return info.clone();
        }
        let info = SignalInfo {
            restart_id: crate::state::new_ulid(),
            signal_at_unix: crate::state::now_unix(),
            in_flight_at_signal: self.in_flight(),
            bound_s,
            at: Instant::now(),
        };
        *guard = Some(info.clone());
        drop(guard);
        self.draining_tx.send_replace(true);
        info
    }

    pub async fn wait_draining(&self) {
        let mut rx = self.draining_tx.subscribe();
        let _ = rx.wait_for(|v| *v).await;
    }

    /// The bound expired: every call still running is interrupted.
    pub fn cancel(&self) {
        self.cancel_tx.send_replace(true);
    }

    /// Resolves when [`Drain::cancel`] fires.
    pub async fn cancelled(&self) {
        let mut rx = self.cancel_tx.subscribe();
        let _ = rx.wait_for(|v| *v).await;
    }

    pub fn note_interrupted(&self) {
        self.interrupted.fetch_add(1, Ordering::SeqCst);
    }

    pub fn interrupted(&self) -> u64 {
        self.interrupted.load(Ordering::SeqCst)
    }
}
