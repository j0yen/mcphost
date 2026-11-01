//! PRD-mcphost-tools-call-progress-notifications: per-call progress bookkeeping
//! (the `calls.progress_token` / `progress_frames` columns) and the
//! `progress_7d` aggregate on admin healthz and `mcphost funnel --json`.

use crate::db::{Db, OriginProgress};
use crate::errors::AppError;
use crate::kinds::{NullProgress, ProgressSink};
use crate::protocol::frames::{self, Frame};
use crate::state::AppState;
use rmcp::model::{ProgressNotificationParam, ProgressToken};
use rmcp::service::{Peer, RoleServer};
use serde_json::{Map, Value, json};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

/// The trailing window `progress_7d` counts over.
pub const WINDOW_7D_SECS: i64 = 7 * 86_400;

/// One report waiting to be sent as a frame.
struct Report {
    /// `pct` as `progress` with total 100, or the report's ordinal with no total.
    progress: f64,
    total: Option<f64>,
    message: Option<String>,
}

/// Leading-edge throttle: a frame is sent at once when at least this long has
/// passed since the last one, otherwise the report waits (a newer one replaces
/// it) until the window ends or the call ends.
pub const MIN_FRAME_GAP: Duration = Duration::from_millis(1000);

/// Send one report as a `notifications/progress` frame; a failure (client
/// gone) is logged at `debug` and never fails the call.
async fn send_frame(call: &CallProgress, peer: &Peer<RoleServer>, token: &ProgressToken, report: Report) {
    let mut param = ProgressNotificationParam::new(token.clone(), report.progress);
    param.total = report.total;
    param.message = report.message;
    match frames::emit(peer, Frame::Progress(param), frames::REQ_WITH_PROGRESS_TOKEN).await {
        Ok(()) => call.frame_sent(),
        Err(e) => tracing::debug!(error = %e, "progress frame not sent"),
    }
}

/// What one `tools/call` has done on the progress path so far; shared by the
/// task-local slot (read when the `calls` row is written) and the sink.
#[derive(Debug, Default)]
pub struct CallProgress {
    has_token: bool,
    frames: AtomicI64,
    reports: AtomicI64,
    tx: Mutex<Option<mpsc::UnboundedSender<Report>>>,
    task: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl CallProgress {
    pub fn new(token: bool) -> Self {
        Self { has_token: token, ..Self::default() }
    }

    /// Count one `notifications/progress` frame actually sent.
    pub fn frame_sent(&self) {
        self.frames.fetch_add(1, Ordering::Relaxed);
    }

    pub fn frames(&self) -> i64 {
        self.frames.load(Ordering::Relaxed)
    }

    /// Start the task that turns this call's reports into frames on `peer`,
    /// echoing `token` exactly as received.
    pub fn start_forwarding(self: &Arc<Self>, peer: Peer<RoleServer>, token: ProgressToken) {
        let (tx, mut rx) = mpsc::unbounded_channel::<Report>();
        let call = Arc::clone(self);
        let task = tokio::spawn(async move {
            let mut last_sent: Option<Instant> = None;
            let mut pending: Option<Report> = None;
            loop {
                let deadline = pending.as_ref().and(last_sent).map(|t| t + MIN_FRAME_GAP);
                tokio::select! {
                    biased;
                    received = rx.recv() => match received {
                        Some(report) => {
                            if last_sent.is_none_or(|t| t.elapsed() >= MIN_FRAME_GAP) {
                                pending = None;
                                last_sent = Some(Instant::now());
                                send_frame(&call, &peer, &token, report).await;
                            } else {
                                pending = Some(report);
                            }
                        }
                        None => {
                            if let Some(report) = pending.take() {
                                send_frame(&call, &peer, &token, report).await;
                            }
                            break;
                        }
                    },
                    () = async { tokio::time::sleep_until(deadline.unwrap_or_else(Instant::now)).await }, if deadline.is_some() => {
                        if let Some(report) = pending.take() {
                            last_sent = Some(Instant::now());
                            send_frame(&call, &peer, &token, report).await;
                        }
                    }
                }
            }
        });
        if let Ok(mut slot) = self.tx.lock() {
            *slot = Some(tx);
        }
        if let Ok(mut slot) = self.task.try_lock() {
            *slot = Some(task);
        }
    }

    fn report(&self, pct: Option<i64>, msg: Option<String>) {
        let ordinal = self.reports.fetch_add(1, Ordering::Relaxed) + 1;
        let (progress, total) = match pct {
            Some(p) => (p as f64, Some(100.0)),
            None => (ordinal as f64, None),
        };
        if let Ok(slot) = self.tx.lock()
            && let Some(tx) = slot.as_ref()
        {
            let _ = tx.send(Report { progress, total, message: msg });
        }
    }

    /// Stop accepting reports and wait until every accepted one is sent, so
    /// the `calls` row and the result both come after the last frame.
    /// Idempotent.
    pub async fn finish(&self) {
        let tx = self.tx.lock().ok().and_then(|mut slot| slot.take());
        drop(tx);
        let task = self.task.lock().await.take();
        if let Some(task) = task {
            let _ = task.await;
        }
    }
}

/// The sink a token-bearing call's tools report to.
pub struct PeerProgressSink(pub Arc<CallProgress>);

impl ProgressSink for PeerProgressSink {
    fn report(&self, pct: Option<i64>, msg: Option<String>) {
        self.0.report(pct, msg);
    }
}

tokio::task_local! {
    static CALL: Arc<CallProgress>;
    static SINK: Arc<dyn ProgressSink>;
}

/// Runs `fut` with `progress` as the current call's slot and, when the
/// request carried a `progressToken`, [`PeerProgressSink`] as its sink.
pub async fn scope<F: std::future::Future>(progress: Arc<CallProgress>, fut: F) -> F::Output {
    let sink: Option<Arc<dyn ProgressSink>> = progress.has_token.then(|| Arc::new(PeerProgressSink(Arc::clone(&progress))) as Arc<dyn ProgressSink>);
    let fut = CALL.scope(progress, fut);
    match sink {
        Some(sink) => SINK.scope(sink, fut).await,
        None => fut.await,
    }
}

/// Runs `fut` with `sink` as the sink of the call in flight (tests).
pub async fn with_sink<F: std::future::Future>(sink: Arc<dyn ProgressSink>, fut: F) -> F::Output {
    SINK.scope(sink, fut).await
}

/// `(progress_token, progress_frames)` for the `calls` row of the call in
/// flight, after every accepted report has been sent; `(0, 0)` outside a
/// `tools/call` (webhooks, resume, drift reruns).
pub async fn call_columns() -> (i64, i64) {
    let Ok(call) = CALL.try_with(Arc::clone) else {
        return (0, 0);
    };
    call.finish().await;
    (i64::from(call.has_token), call.frames())
}

/// Finish the current call's frames (idempotent); `call_tool` runs this
/// before it returns so a result never overtakes a frame.
pub async fn finish_current() {
    if let Ok(call) = CALL.try_with(Arc::clone) {
        call.finish().await;
    }
}

/// Build `{<origin>: n, ..., total: n}` over the shared key set.
fn counts_object(keys: &[String], pick: impl Fn(&str) -> i64) -> Value {
    let mut obj = Map::new();
    let mut total = 0;
    for key in keys {
        let n = pick(key);
        total += n;
        obj.insert(key.clone(), json!(n));
    }
    obj.insert("total".to_string(), json!(total));
    Value::Object(obj)
}

/// The `progress_7d` object: every completed `tools/call` row in the last 7
/// days split by the tenant's stored origin, the share that carried a
/// `progressToken`, and the frames sent. All three share one key set.
pub async fn compute(db: &Db, now: i64) -> Result<Value, AppError> {
    let rows = db.progress_counts_by_origin(now - WINDOW_7D_SECS).await?;
    let mut keys: Vec<String> = vec!["synthetic".to_string(), "external".to_string()];
    for row in &rows {
        if !keys.contains(&row.origin) {
            keys.push(row.origin.clone());
        }
    }
    let of = |origin: &str, col: fn(&OriginProgress) -> i64| rows.iter().find(|r| r.origin == origin).map(col).unwrap_or(0);
    Ok(json!({
        "calls_total": counts_object(&keys, |k| of(k, |r| r.calls)),
        "calls_with_token": counts_object(&keys, |k| of(k, |r| r.with_token)),
        "frames_sent": counts_object(&keys, |k| of(k, |r| r.frames)),
        "since": db.progress_epoch().await?,
    }))
}

/// Admin healthz's `progress_7d`: [`compute`] behind the same 60 s cache
/// contract as `oauth` ([`crate::oauth_stats::healthz_json`]); a storage error
/// falls back to an all-zero object.
pub async fn healthz_json(state: &AppState) -> Value {
    let now = crate::state::now_unix();
    if let Ok(guard) = state.progress_healthz_cache.lock()
        && let Some(cached) = guard.as_ref()
        && now - cached.fetched_at < crate::oauth_stats::HEALTHZ_CACHE_TTL_SECS
    {
        return cached.value.clone();
    }
    let value = match compute(&state.db, now).await {
        Ok(v) => v,
        Err(_) => json!({
            "calls_total": {"synthetic": 0, "external": 0, "total": 0},
            "calls_with_token": {"synthetic": 0, "external": 0, "total": 0},
            "frames_sent": {"synthetic": 0, "external": 0, "total": 0},
            "since": 0,
        }),
    };
    if let Ok(mut guard) = state.progress_healthz_cache.lock() {
        *guard = Some(crate::oauth_stats::CachedHealthz { value: value.clone(), fetched_at: now });
    }
    value
}

/// `host.probe.progress` bounds: the longest run (5 x 5 s = 25 s) stays under
/// `state::CALL_TIMEOUT` (30 s).
pub const PROBE_STEPS: std::ops::RangeInclusive<i64> = 1..=5;
pub const PROBE_STEP_MS: std::ops::RangeInclusive<i64> = 1000..=5000;

/// How `host.probe.progress` reports: a percentage per step, or a bare step
/// count with no total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeMode {
    Pct,
    Count,
}

impl ProbeMode {
    /// The `mode` property's allowed values, for the descriptor schema.
    pub const NAMES: [&'static str; 2] = ["pct", "count"];
}

fn bounded(args: &Value, key: &str, range: &std::ops::RangeInclusive<i64>) -> Result<i64, AppError> {
    let n = args.get(key).and_then(Value::as_i64).ok_or_else(|| AppError::InvalidArgs(format!("{key} is required and must be an integer")))?;
    if !range.contains(&n) {
        return Err(AppError::InvalidArgs(format!("{key} must be {}..={}, got {n}", range.start(), range.end())));
    }
    Ok(n)
}

/// `host.probe.progress {steps, step_ms, mode?}`: sleeps `step_ms` per step and
/// reports through `sink` after each, so the whole progress path can be
/// exercised without a tenant tool. Returns `{steps, elapsed_ms}`.
pub async fn probe(args: &Value, sink: &dyn ProgressSink) -> Result<Value, AppError> {
    let steps = bounded(args, "steps", &PROBE_STEPS)?;
    let step_ms = bounded(args, "step_ms", &PROBE_STEP_MS)?;
    let mode = match args.get("mode").and_then(Value::as_str) {
        None | Some("pct") => ProbeMode::Pct,
        Some("count") => ProbeMode::Count,
        Some(other) => return Err(AppError::InvalidArgs(format!("mode must be one of {:?}, got {other}", ProbeMode::NAMES))),
    };
    let started = std::time::Instant::now();
    for i in 1..=steps {
        tokio::time::sleep(std::time::Duration::from_millis(step_ms as u64)).await;
        match mode {
            ProbeMode::Pct => sink.report(Some(i * 100 / steps), Some(format!("step {i}/{steps}"))),
            ProbeMode::Count => sink.report(None, Some(format!("step {i}"))),
        }
    }
    Ok(json!({"steps": steps, "elapsed_ms": started.elapsed().as_millis() as i64}))
}

/// The sink the call in flight reports to: the peer sink when the request
/// carried a `progressToken`, else [`crate::kinds::NullProgress`].
pub fn current_sink() -> Arc<dyn ProgressSink> {
    SINK.try_with(Arc::clone).unwrap_or_else(|_| Arc::new(NullProgress))
}
