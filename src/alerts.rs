//! PRD-mcphost-sqlite-busy-timeout-audit requirement 6 (P1, AC9): a
//! minimal, in-process alerting source registry -- "the alerting source
//! registry is present" is unconditionally true here (this crate's own
//! registry; "no hard dependency" means no *external* one, not that this
//! PRD builds nothing), and [`ContentionTracker`] decides when
//! `db_busy_total`'s rise over a trailing window earns a `db.contention`
//! alert.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

#[derive(Debug, Clone)]
pub struct AlertEvent {
    pub key: &'static str,
    pub detail: Value,
    pub raised_unix: i64,
}

/// Every alert this process has ever raised, in memory -- `admin.*` has no
/// reader for this yet (out of this PRD's scope beyond AC9's own test,
/// which reads [`AlertRegistry::events`] directly); logged at `warn` on
/// every raise so it's visible without one.
#[derive(Clone, Default)]
pub struct AlertRegistry {
    events: Arc<Mutex<Vec<AlertEvent>>>,
}

impl AlertRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn raise(&self, key: &'static str, detail: Value) {
        let event = AlertEvent {
            key,
            detail: detail.clone(),
            raised_unix: crate::state::now_unix(),
        };
        tracing::warn!(alert_key = key, detail = %detail, "alert raised");
        self.events.lock().unwrap().push(event);
    }

    pub fn events(&self) -> Vec<AlertEvent> {
        self.events.lock().unwrap().clone()
    }
}

/// Requirement 6: "`db_busy_total` rises by ≥ 10 within 5 min" -- a
/// trailing window of (timestamp, `db_busy_total`) samples, one appended
/// per minute tick.
const WINDOW_SECS: i64 = 300;
const RISE_THRESHOLD: u64 = 10;

/// One alert per contiguous rise-episode, not one per tick while still
/// elevated: `active` latches once the threshold trips and only clears
/// once a tick's rise drops back under it.
#[derive(Clone, Default)]
pub struct ContentionTracker {
    samples: Arc<Mutex<VecDeque<(i64, u64)>>>,
    active: Arc<AtomicBool>,
}

impl ContentionTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one minute-tick sample and returns `Some(rise)` the first
    /// tick this window's rise crosses [`RISE_THRESHOLD`].
    fn tick(&self, now_unix: i64, busy_total_now: u64) -> Option<u64> {
        let mut samples = self.samples.lock().unwrap();
        samples.push_back((now_unix, busy_total_now));
        while let Some(&(ts, _)) = samples.front() {
            if now_unix - ts > WINDOW_SECS {
                samples.pop_front();
            } else {
                break;
            }
        }
        let oldest = samples.front().map(|&(_, v)| v).unwrap_or(busy_total_now);
        let rise = busy_total_now.saturating_sub(oldest);
        if rise >= RISE_THRESHOLD {
            if self.active.swap(true, Ordering::Relaxed) {
                None
            } else {
                Some(rise)
            }
        } else {
            self.active.store(false, Ordering::Relaxed);
            None
        }
    }
}

/// One minute-tick cycle: sum `db_busy_total` across every [`crate::db::DbRole`],
/// feed it to `state.contention_tracker`, and raise `db.contention` through
/// `state.alerts` on a fresh rise. Exposed as a single deterministic step
/// (same "swap a short interval in for the real cadence" convention as
/// `retention::spawn_prune_scheduler_for_test`) so a test doesn't wait out
/// the real 60s loop.
pub fn tick_once(state: &crate::state::AppState) {
    let now = crate::state::now_unix();
    let busy_total: u64 = crate::db::ALL_ROLES
        .iter()
        .map(|&role| state.db.counters(role).busy_total)
        .sum();
    if let Some(rise) = state.contention_tracker.tick(now, busy_total) {
        state.alerts.raise(
            "db.contention",
            serde_json::json!({
                "rise": rise,
                "window_secs": WINDOW_SECS,
                "busy_total": busy_total,
            }),
        );
    }
}

pub fn spawn_tick(state: crate::state::AppState) -> tokio::task::JoinHandle<()> {
    spawn_tick_loop(state, None)
}

/// Test-only: same loop `spawn_tick` runs at real `serve` startup, but
/// firing every `interval` instead of waiting out the real 60s cadence.
pub fn spawn_tick_for_test(
    state: crate::state::AppState,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    spawn_tick_loop(state, Some(interval))
}

fn spawn_tick_loop(
    state: crate::state::AppState,
    interval_override: Option<Duration>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(interval_override.unwrap_or(Duration::from_secs(60))).await;
            tick_once(&state);
        }
    })
}
