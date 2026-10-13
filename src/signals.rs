//! PRD-mcphost-golden-signals-self-probe: five golden signals the host
//! samples about itself every minute, graded against the thresholds in the
//! compiled `heal.toml`. This module is the band logic ([`Tracker`]) and the
//! config ([`HealConfig`]); nothing here acts on a breach.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;

use serde::Deserialize;

/// R1: 24 h of one-minute samples per signal.
pub const RING_CAPACITY: usize = 1440;

/// The compiled-in thresholds (R3).
pub const COMPILED_HEAL_TOML: &str = include_str!("../heal.toml");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Signal {
    EchoP95Ms,
    ColdStartP50Ms,
    SqliteWriteMs,
    DiskFreeMb,
    SandboxPool,
}

impl Signal {
    pub const ALL: [Signal; 5] = [
        Signal::EchoP95Ms,
        Signal::ColdStartP50Ms,
        Signal::SqliteWriteMs,
        Signal::DiskFreeMb,
        Signal::SandboxPool,
    ];

    /// The key used in `heal.toml`, `/status.json.signals` and log lines.
    pub fn name(self) -> &'static str {
        match self {
            Signal::EchoP95Ms => "echo_p95_ms",
            Signal::ColdStartP50Ms => "cold_start_p50_ms",
            Signal::SqliteWriteMs => "sqlite_write_ms",
            Signal::DiskFreeMb => "disk_free_mb",
            Signal::SandboxPool => "sandbox_pool",
        }
    }

    pub fn from_name(name: &str) -> Option<Signal> {
        Signal::ALL.into_iter().find(|s| s.name() == name)
    }

    /// `true` when a smaller value is the bad direction (free disk).
    fn lower_is_worse(self) -> bool {
        matches!(self, Signal::DiskFreeMb)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Band {
    Green,
    Amber,
    Red,
    Unknown,
}

impl Band {
    pub fn as_str(self) -> &'static str {
        match self {
            Band::Green => "green",
            Band::Amber => "amber",
            Band::Red => "red",
            Band::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Threshold {
    pub amber: f64,
    pub red: f64,
    pub for_s: i64,
    pub unit: String,
    #[serde(default)]
    pub wedge_factor: Option<f64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HealFile {
    signals: BTreeMap<String, Threshold>,
}

#[derive(Clone, Debug)]
pub struct HealConfig {
    thresholds: BTreeMap<Signal, Threshold>,
}

impl HealConfig {
    /// Parses a `heal.toml`. Every [`Signal::ALL`] key must be present and no
    /// other key may be: a partial or extended file is rejected, not merged.
    pub fn parse(text: &str) -> Result<HealConfig, String> {
        let file: HealFile = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut thresholds = BTreeMap::new();
        for (name, t) in file.signals {
            let signal = Signal::from_name(&name).ok_or_else(|| format!("unknown signal `{name}`"))?;
            thresholds.insert(signal, t);
        }
        for signal in Signal::ALL {
            if !thresholds.contains_key(&signal) {
                return Err(format!("missing signal `{}`", signal.name()));
            }
        }
        Ok(HealConfig { thresholds })
    }

    /// The compiled file. A test (`gsig_ac01`) and the boot path both depend
    /// on it parsing, so a malformed `heal.toml` is a build-time-visible bug.
    pub fn compiled() -> HealConfig {
        HealConfig::parse(COMPILED_HEAL_TOML).expect("compiled heal.toml parses")
    }

    /// `override_path` parsed over the compiled file, or the compiled file
    /// itself when there is no override or it is rejected.
    pub fn from_override(override_path: Option<&std::ffi::OsStr>) -> HealConfig {
        let Some(path) = override_path.map(std::path::Path::new) else {
            return HealConfig::compiled();
        };
        match std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|text| HealConfig::parse(&text))
        {
            Ok(config) => config,
            Err(reason) => {
                tracing::warn!(path = %path.display(), %reason, "heal.toml override rejected; using the compiled thresholds");
                HealConfig::compiled()
            }
        }
    }

    pub fn threshold(&self, signal: Signal) -> &Threshold {
        &self.thresholds[&signal]
    }

    /// Grades `value`: red at or past `red`, amber at or past `amber`
    /// (inverted for [`Signal::lower_is_worse`]), else green.
    pub fn band(&self, signal: Signal, value: f64) -> Band {
        let t = self.threshold(signal);
        let (red, amber) = if signal.lower_is_worse() {
            (value <= t.red, value <= t.amber)
        } else {
            (value >= t.red, value >= t.amber)
        };
        if red {
            Band::Red
        } else if amber {
            Band::Amber
        } else {
            Band::Green
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub ts: i64,
    pub value: f64,
    pub band: Band,
}

/// What a signal crossing emits: one breach per red stretch, one clear when
/// it is back to green.
#[derive(Clone, Debug, PartialEq)]
pub enum SignalEvent {
    Breach {
        signal: Signal,
        value: f64,
        threshold: f64,
        since: i64,
    },
    Clear {
        signal: Signal,
        value: f64,
        threshold: f64,
        at: i64,
    },
}

/// One signal's ring plus the state machine that turns bands into events.
#[derive(Debug)]
pub struct SignalState {
    pub ring: VecDeque<Sample>,
    /// Start of the current band.
    pub band_since: i64,
    red_since: Option<i64>,
    breach_open: bool,
}

impl Default for SignalState {
    fn default() -> Self {
        SignalState {
            ring: VecDeque::with_capacity(RING_CAPACITY),
            band_since: 0,
            red_since: None,
            breach_open: false,
        }
    }
}

impl SignalState {
    /// Records one sample and returns the event it triggers, if any. The
    /// breach fires once `for_s` seconds after the first red sample and stays
    /// open through amber; it clears only on a green sample.
    pub fn observe(&mut self, config: &HealConfig, signal: Signal, ts: i64, value: f64) -> Option<SignalEvent> {
        let band = config.band(signal, value);
        if self.ring.back().map(|s| s.band) != Some(band) {
            self.band_since = ts;
        }
        if self.ring.len() == RING_CAPACITY {
            self.ring.pop_front();
        }
        self.ring.push_back(Sample { ts, value, band });

        let t = config.threshold(signal);
        if band == Band::Red {
            let since = *self.red_since.get_or_insert(ts);
            if !self.breach_open && ts - since >= t.for_s {
                self.breach_open = true;
                return Some(SignalEvent::Breach { signal, value, threshold: t.red, since });
            }
        } else {
            self.red_since = None;
            if self.breach_open && band == Band::Green {
                self.breach_open = false;
                return Some(SignalEvent::Clear { signal, value, threshold: t.amber, at: ts });
            }
        }
        None
    }
}

/// One `{"ts":…,"signal":"…","value":…}` line of a samples file.
#[derive(Deserialize)]
struct ReplayLine {
    ts: i64,
    signal: String,
    value: f64,
}

/// R7: feeds recorded samples through the band logic and returns the events
/// in order. Lines must be in timestamp order, as a recording is.
pub fn replay(config: &HealConfig, jsonl: &str) -> Result<Vec<SignalEvent>, String> {
    let mut states: BTreeMap<Signal, SignalState> = BTreeMap::new();
    let mut events = Vec::new();
    for (i, line) in jsonl.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
        let row: ReplayLine = serde_json::from_str(line).map_err(|e| format!("line {}: {e}", i + 1))?;
        let signal =
            Signal::from_name(&row.signal).ok_or_else(|| format!("line {}: unknown signal `{}`", i + 1, row.signal))?;
        if let Some(ev) = states.entry(signal).or_default().observe(config, signal, row.ts, row.value) {
            events.push(ev);
        }
    }
    Ok(events)
}

/// The line `mcphost signals replay` prints for an event.
pub fn render_event(ev: &SignalEvent) -> String {
    use crate::state::rfc3339_from_unix;
    match ev {
        SignalEvent::Breach { signal, value, threshold, since } => format!(
            "breach signal={} since={} value={value} threshold={threshold}",
            signal.name(),
            rfc3339_from_unix(*since)
        ),
        SignalEvent::Clear { signal, value, threshold, at } => format!(
            "clear signal={} at={} value={value} threshold={threshold}",
            signal.name(),
            rfc3339_from_unix(*at)
        ),
    }
}

/// The window the echo and cold-start percentiles and `samples_5m` look at.
pub const WINDOW_SECS: i64 = 300;

/// Nearest-rank percentile (`pct` in 0..=100) of `values`; `0.0` when empty.
pub fn percentile(values: &[f64], pct: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let rank = ((sorted.len() as f64) * pct / 100.0).ceil() as usize;
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}

#[derive(Default)]
struct Inner {
    states: BTreeMap<Signal, SignalState>,
    /// Echo-probe call durations `(unix ts, ms)` from the last
    /// [`WINDOW_SECS`].
    echo_raw: VecDeque<(i64, f64)>,
}

/// R1: the per-signal rings and the thresholds they are graded against,
/// shared through `AppState`.
#[derive(Clone)]
pub struct Signals {
    inner: Arc<Mutex<Inner>>,
    config: Arc<HealConfig>,
    events: broadcast::Sender<SignalEvent>,
}

/// Events a slow subscriber may lag behind by before it starts losing them.
const EVENT_CHANNEL_CAPACITY: usize = 64;

impl Signals {
    pub fn new(config: HealConfig) -> Signals {
        Signals {
            inner: Arc::default(),
            config: Arc::new(config),
            events: broadcast::channel(EVENT_CHANNEL_CAPACITY).0,
        }
    }

    /// R3: the boot path. `MCPHOST_HEAL_TOML` names a file that replaces the
    /// compiled thresholds; one that is unreadable, malformed, or does not
    /// name exactly [`Signal::ALL`] is logged and ignored.
    pub fn from_env() -> Signals {
        Signals::new(HealConfig::from_override(std::env::var_os("MCPHOST_HEAL_TOML").as_deref()))
    }

    /// The compiled `heal.toml`.
    pub fn compiled() -> Signals {
        Signals::new(HealConfig::compiled())
    }

    pub fn config(&self) -> &HealConfig {
        &self.config
    }

    /// Records the echo probe's call durations for this tick.
    pub fn record_echo_calls(&self, ts: i64, durations_ms: &[f64]) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.echo_raw.extend(durations_ms.iter().map(|ms| (ts, *ms)));
        while inner.echo_raw.front().is_some_and(|(t, _)| ts - t > WINDOW_SECS) {
            inner.echo_raw.pop_front();
        }
    }

    /// p95 of the echo calls in the last [`WINDOW_SECS`].
    pub fn echo_p95_ms(&self) -> f64 {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let v: Vec<f64> = inner.echo_raw.iter().map(|(_, ms)| *ms).collect();
        percentile(&v, 95.0)
    }

    /// R5: a receiver of every [`SignalEvent`] from now on. Each subscriber
    /// gets its own copy of each event.
    pub fn subscribe(&self) -> broadcast::Receiver<SignalEvent> {
        self.events.subscribe()
    }

    /// Records one sample; when it triggers an event, logs the transition
    /// line, broadcasts it, and returns it.
    pub fn record(&self, signal: Signal, ts: i64, value: f64) -> Option<SignalEvent> {
        let event = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.states.entry(signal).or_default().observe(&self.config, signal, ts, value)
        };
        if let Some(ev) = &event {
            log_event(ev);
            // No subscriber is not an error: the eyes work without actuators.
            let _ = self.events.send(ev.clone());
        }
        event
    }

    /// R4: `/status.json.signals`.
    pub fn status_json(&self, now: i64) -> serde_json::Value {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = serde_json::Map::new();
        for signal in Signal::ALL {
            let t = self.config.threshold(signal);
            let state = inner.states.get(&signal);
            let ring = state.map(|s| &s.ring);
            let n = ring.map_or(0, VecDeque::len);
            let last = ring.and_then(|r| r.back());
            let known = n >= 2;
            out.insert(
                signal.name().to_string(),
                serde_json::json!({
                    "value": last.filter(|_| known).map(|s| s.value),
                    "unit": t.unit,
                    "band": if known { last.map_or(Band::Unknown, |s| s.band) } else { Band::Unknown }.as_str(),
                    "since_unix": state.filter(|_| known).map(|s| s.band_since),
                    "amber": threshold_json(t.amber),
                    "red": threshold_json(t.red),
                    "samples_5m": ring.map_or(0, |r| r.iter().filter(|s| now - s.ts <= WINDOW_SECS).count()),
                }),
            );
        }
        serde_json::Value::Object(out)
    }
}

/// A threshold as `heal.toml` wrote it: `2000`, not `2000.0`, when whole.
fn threshold_json(v: f64) -> serde_json::Value {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        serde_json::json!(v as i64)
    } else {
        serde_json::json!(v)
    }
}

/// R5: the tracing line for a breach or clear.
fn log_event(ev: &SignalEvent) {
    match ev {
        SignalEvent::Breach { signal, value, threshold, .. } => {
            tracing::warn!("signal={} band=red value={value} threshold={threshold}", signal.name());
        }
        SignalEvent::Clear { signal, value, threshold, .. } => {
            tracing::info!("signal={} band=green value={value} threshold={threshold}", signal.name());
        }
    }
}
