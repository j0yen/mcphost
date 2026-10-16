//! PRD-mcphost-incident-banner-for-agents: while an incident is open, every
//! host-fault error ([`crate::errors::HOST_FAULT_CODES`]) carries
//! `data.incident {id, since, status_url, impact, retry_after_s}`, and a
//! 5-minute sliding window over handler results opens an
//! `auto:error_rate` incident from real traffic.
//!
//! The banner and the window are keyed by the host's `public_url` -- the
//! same base every `into_error_data_at` call site already passes -- so the
//! error hot path never queries SQLite and never needs an `AppState`, and
//! the parallel test servers in one binary never see each other's banner.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, RwLock};

use serde_json::{Value, json};

use crate::errors::{AppError, HOST_FAULT_CODES, INCIDENT_FIELD};
use crate::state::{AppState, now_unix, rfc3339_from_unix};

/// R3: sliding-window length.
pub const WINDOW_SECS: i64 = 300;
/// R3: host-fault results in the window needed to open.
pub const ERROR_RATE_OPEN_MIN_RESULTS: u32 = 20;
/// R3: host-fault share of all results needed to open.
pub const ERROR_RATE_OPEN_RATIO: f64 = 0.05;
/// R3: the share below which a minute counts as calm.
pub const ERROR_RATE_CLOSE_RATIO: f64 = 0.01;
/// R3: consecutive calm seconds before the auto-incident closes.
pub const ERROR_RATE_CLOSE_CALM_SECS: i64 = 600;
/// R5: `retry_after_s` for `auto:*` incidents.
pub const AUTO_RETRY_AFTER_S: u64 = 30;
/// Title prefix of the incident this module opens.
pub const ERROR_RATE_TITLE_PREFIX: &str = "auto:error_rate";

/// R4: the newest open incident, as errors and `/status.json` report it.
#[derive(Debug, Clone, PartialEq)]
pub struct Banner {
    pub id: i64,
    pub opened_at: i64,
    pub impact: String,
    pub status_url: String,
    pub retry_after_s: u64,
}

impl Banner {
    /// The `data.incident` object on a host-fault error.
    pub fn to_error_json(&self) -> Value {
        json!({
            "id": self.id,
            "since": rfc3339_from_unix(self.opened_at),
            "status_url": self.status_url,
            "impact": self.impact,
            "retry_after_s": self.retry_after_s,
        })
    }
}

/// R4: the `/status.json` `banner` value for the newest open incident.
pub fn banner_json(public_url: &str, newest: Option<&crate::db::Incident>) -> Value {
    match newest {
        Some(i) => json!({
            "id": i.id,
            "since": rfc3339_from_unix(i.opened_at),
            "impact": i.impact,
            "status_url": format!("{}/status.html#{}", public_url.trim_end_matches('/'), i.id),
        }),
        None => Value::Null,
    }
}

static BANNERS: LazyLock<RwLock<HashMap<String, Banner>>> = LazyLock::new(Default::default);

/// The open-incident banner for the host serving `base_url`, if any.
pub fn banner_for(base_url: &str) -> Option<Banner> {
    BANNERS.read().unwrap_or_else(|e| e.into_inner()).get(base_url).cloned()
}

fn set_banner(base_url: &str, banner: Option<Banner>) {
    let mut map = BANNERS.write().unwrap_or_else(|e| e.into_inner());
    match banner {
        Some(b) => map.insert(base_url.to_string(), b),
        None => map.remove(base_url),
    };
}

/// Re-reads the newest open incident into the banner cell. Called by the
/// statusfeed tick and after every `admin.incident.*` write.
pub async fn refresh(state: &AppState) -> Result<(), AppError> {
    let newest = state.db.list_open_incidents().await?.into_iter().next();
    let banner = newest.map(|i| Banner {
        id: i.id,
        opened_at: i.opened_at,
        impact: i.impact,
        status_url: format!("{}/status.html#{}", state.public_url.trim_end_matches('/'), i.id),
        retry_after_s: AUTO_RETRY_AFTER_S,
    });
    set_banner(&state.public_url, banner);
    Ok(())
}

#[derive(Default)]
struct Bucket {
    ts: i64,
    all: u32,
    host_fault: u32,
    by_code: HashMap<String, u32>,
}

struct Window {
    buckets: Vec<Bucket>,
    calm_since: Option<i64>,
}

impl Default for Window {
    fn default() -> Self {
        Self { buckets: (0..WINDOW_SECS).map(|_| Bucket::default()).collect(), calm_since: None }
    }
}

static WINDOWS: LazyLock<Mutex<HashMap<String, Window>>> = LazyLock::new(Default::default);

/// Records one handler result at `now`: `error_code` is the result's
/// `data.error_code`, `None` for a success.
pub fn record_result(base_url: &str, now: i64, error_code: Option<&str>) {
    let mut windows = WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
    let window = windows.entry(base_url.to_string()).or_default();
    let bucket = &mut window.buckets[now.rem_euclid(WINDOW_SECS) as usize];
    if bucket.ts != now {
        *bucket = Bucket { ts: now, ..Bucket::default() };
    }
    bucket.all += 1;
    if let Some(code) = error_code.filter(|c| HOST_FAULT_CODES.contains(c)) {
        bucket.host_fault += 1;
        *bucket.by_code.entry(code.to_string()).or_default() += 1;
    }
}

/// What the window says at one instant.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowReading {
    pub all: u32,
    pub host_fault: u32,
    pub dominant_code: Option<String>,
}

impl WindowReading {
    pub fn ratio(&self) -> f64 {
        if self.all == 0 { 0.0 } else { f64::from(self.host_fault) / f64::from(self.all) }
    }

    pub fn should_open(&self) -> bool {
        self.host_fault >= ERROR_RATE_OPEN_MIN_RESULTS && self.ratio() >= ERROR_RATE_OPEN_RATIO
    }
}

fn read_window(window: &Window, now: i64) -> WindowReading {
    let mut reading = WindowReading { all: 0, host_fault: 0, dominant_code: None };
    let mut by_code: HashMap<&str, u32> = HashMap::new();
    for b in window.buckets.iter().filter(|b| b.ts > now - WINDOW_SECS && b.ts <= now) {
        reading.all += b.all;
        reading.host_fault += b.host_fault;
        for (code, n) in &b.by_code {
            *by_code.entry(code).or_default() += n;
        }
    }
    reading.dominant_code = by_code
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(c, _)| c.to_string());
    reading
}

/// The window's current reading for `base_url`.
pub fn reading(base_url: &str, now: i64) -> WindowReading {
    let windows = WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
    match windows.get(base_url) {
        Some(w) => read_window(w, now),
        None => WindowReading { all: 0, host_fault: 0, dominant_code: None },
    }
}

/// R3: evaluates the window at `now`. Opens an `auto:error_rate <code> <pct>%`
/// incident (through [`crate::statusfeed::open_auto_incident_titled`], the
/// self-probe's own path) when the open rule holds and no error-rate
/// incident is already open; closes it after
/// [`ERROR_RATE_CLOSE_CALM_SECS`] of window share under
/// [`ERROR_RATE_CLOSE_RATIO`]. Refreshes the banner either way.
pub async fn evaluate_error_rate(state: &AppState, now: i64) -> Result<(), AppError> {
    let (read, calm_for) = {
        let mut windows = WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
        let window = windows.entry(state.public_url.clone()).or_default();
        let read = read_window(window, now);
        if read.ratio() < ERROR_RATE_CLOSE_RATIO {
            let since = *window.calm_since.get_or_insert(now);
            (read, Some(now - since))
        } else {
            window.calm_since = None;
            (read, None)
        }
    };
    let open_error_rate = state
        .db
        .list_open_incidents()
        .await?
        .into_iter()
        .find(|i| i.auto && i.title.starts_with(ERROR_RATE_TITLE_PREFIX));
    match open_error_rate {
        None if read.should_open() => {
            let code = read.dominant_code.clone().unwrap_or_else(|| "internal".to_string());
            let title = format!("{ERROR_RATE_TITLE_PREFIX} {code} {:.1}%", read.ratio() * 100.0);
            crate::statusfeed::open_auto_incident_titled(state, &code, &title).await?;
        }
        Some(incident) if calm_for.is_some_and(|s| s >= ERROR_RATE_CLOSE_CALM_SECS) => {
            crate::statusfeed::close_auto_incident_with(
                state,
                &incident,
                "auto-resolved: auto:error_rate trigger closed it, host-fault share under 1% for 10 minutes",
            )
            .await?;
        }
        _ => {}
    }
    refresh(state).await
}

/// Wall-clock convenience for the tick.
pub async fn tick(state: &AppState) -> Result<(), AppError> {
    evaluate_error_rate(state, now_unix()).await
}

/// The `data.error_code` of a JSON-RPC error, for [`record_result`].
pub fn error_code_of(data: &Option<Value>) -> Option<String> {
    data.as_ref()?.get("error_code")?.as_str().map(str::to_string)
}

tokio::task_local! {
    /// R7: the id of the incident a response of this request carried; set
    /// by [`attach`]/[`decorate_http_body`], read by the request-log
    /// middleware that scopes it.
    pub static ATTACHED: std::cell::Cell<Option<i64>>;
}

fn note_attached(id: i64) {
    let _ = ATTACHED.try_with(|c| c.set(Some(id)));
}

/// Attaches the banner (if any) to an error `obj` whose code is host-fault.
pub fn attach(obj: &mut serde_json::Map<String, Value>, code: &str, base_url: Option<&str>) {
    if !HOST_FAULT_CODES.contains(&code) {
        return;
    }
    if let Some(banner) = base_url.and_then(banner_for) {
        note_attached(banner.id);
        obj.insert(INCIDENT_FIELD.to_string(), banner.to_error_json());
    }
}

/// R2/R5 for plain HTTP routes: on a 5xx while an incident is open, returns
/// the `Retry-After` seconds to set, and for a host-fault `code` adds
/// `data.<INCIDENT_FIELD>` to the JSON `body`.
pub fn decorate_http_body(base_url: &str, status: u16, code: &str, body: &mut Value) -> Option<u64> {
    if !(500..600).contains(&status) {
        return None;
    }
    let banner = banner_for(base_url)?;
    if HOST_FAULT_CODES.contains(&code)
        && let Some(obj) = body.as_object_mut()
    {
        note_attached(banner.id);
        obj.insert("data".to_string(), json!({ INCIDENT_FIELD: banner.to_error_json() }));
    }
    Some(banner.retry_after_s)
}

/// Test hook: forget every result recorded for `base_url`. The window is
/// process-global, so a `TestServer::start` that performs host calls of its
/// own leaves records in the window before a test lays down its fixture
/// (incban_ac01 read 20/103 = 19.4 % instead of 20/100 after such a start).
#[doc(hidden)]
pub fn reset_window_for_tests(base_url: &str) {
    let mut windows = WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
    windows.remove(base_url);
}
