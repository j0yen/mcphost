//! PRD-mcphost-call-trace-replay: a bounded, sanitized trace per tenant tool
//! call, addressable by the `calls.id` the call's result returns as
//! `call_id`.
//!
//! Three small pieces live here:
//!
//! * a per-call [`Capture`] the python kind fills in while it runs (output
//!   tails, the runner's phase stamps, when the payload was handed to the
//!   sandbox and when the answer came back). It travels in a tokio task-local,
//!   the same way `dryrun` scopes a savepoint, so `Kind::call`'s signature and
//!   every `CallCtx` construction stay untouched;
//! * [`Redactor`], which masks secret-looking argument keys and tenant secret
//!   values before anything is stored;
//! * the call-id slot `host.tool.call` reads back after dispatch.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::{Value, json};

/// stdout/stderr tail bound (requirement 2).
pub const TAIL_BYTES: usize = 4096;
/// `request_json` bound (requirement 2).
pub const REQUEST_BYTES: usize = 8192;
/// Retention: newest N traces per tenant ...
pub const RETAIN_PER_TENANT: i64 = 200;
/// ... and none older than this many seconds.
pub const RETAIN_SECS: i64 = 86_400;

static ENABLED: AtomicBool = AtomicBool::new(true);

/// Whether new calls are traced. On by default; `MCPHOST_CALL_TRACE=0` (read
/// once at startup by [`init_from_env`]) turns it off, the sampling escape
/// hatch the PRD's falsifier asks for if the cost ever exceeds its budget.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn init_from_env() {
    if matches!(std::env::var("MCPHOST_CALL_TRACE").ok().as_deref(), Some("0" | "off" | "false")) {
        set_enabled(false);
    }
}

/// What a kind learned about one call while running it.
pub struct Capture {
    t_call: Instant,
    dispatched: Option<Instant>,
    responded: Option<Instant>,
    stdout: String,
    stderr: String,
    /// `(args_ms, tool_code_ms, serialize_ms)` as the python runner stamped them.
    runner: Option<(f64, f64, f64)>,
    /// Phase timings a kind reports whole (the http kind).
    phases: Option<Value>,
}

pub type SharedCapture = Arc<Mutex<Capture>>;

tokio::task_local! {
    static CAPTURE: SharedCapture;
    static CALL_ID: Arc<AtomicI64>;
}

pub fn new_capture() -> SharedCapture {
    Arc::new(Mutex::new(Capture {
        t_call: Instant::now(),
        dispatched: None,
        responded: None,
        stdout: String::new(),
        stderr: String::new(),
        runner: None,
        phases: None,
    }))
}

/// Runs `fut` with `cap` as the current call's capture.
pub async fn scope<F: Future>(cap: SharedCapture, fut: F) -> F::Output {
    CAPTURE.scope(cap, fut).await
}

fn with<R>(f: impl FnOnce(&mut Capture) -> R) -> Option<R> {
    CAPTURE
        .try_with(|c| {
            let mut guard = c.lock().unwrap_or_else(|e| e.into_inner());
            f(&mut guard)
        })
        .ok()
}

/// The last `max` bytes of `s`, cut on a char boundary.
pub fn tail_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

/// The payload is about to be handed to the sandbox: everything before this
/// is host-side admission (`queue_ms`). Called again by a cold fallback, so
/// the latest handoff wins.
pub fn mark_dispatch() {
    with(|c| c.dispatched = Some(Instant::now()));
}

/// A parsed runner envelope came back: its output captures, its phase
/// stamps, and (for an exception) the traceback, which is what the tool
/// "wrote to stderr" last.
pub fn note_envelope(envelope: &Value) {
    if CAPTURE.try_with(|_| ()).is_err() {
        return;
    }
    let text = |key: &str| envelope.get(key).and_then(Value::as_str).unwrap_or("");
    let mut stderr = text("stderr_capture").to_string();
    let traceback = text("traceback");
    if envelope.get("ok").and_then(Value::as_bool) != Some(true) && !traceback.is_empty() {
        if !stderr.is_empty() && !stderr.ends_with('\n') {
            stderr.push('\n');
        }
        stderr.push_str(traceback);
    }
    let stdout = tail_bytes(text("stdout_capture"), TAIL_BYTES).to_string();
    let stderr = tail_bytes(&stderr, TAIL_BYTES).to_string();
    let phase = |t: &Value, key: &str| t.get(key).and_then(Value::as_f64);
    let runner = envelope.get("timings").and_then(|t| {
        Some((phase(t, "args_ms")?, phase(t, "tool_code_ms")?, phase(t, "serialize_ms")?))
    });
    with(|c| {
        c.responded = Some(Instant::now());
        c.stdout = stdout;
        c.stderr = stderr;
        c.runner = runner;
    });
}

/// The sandbox died without a readable envelope: the process's own tails are
/// all there is.
pub fn note_process_tails(stdout: &str, stderr: &str) {
    if CAPTURE.try_with(|_| ()).is_err() {
        return;
    }
    let stdout = tail_bytes(stdout, TAIL_BYTES).to_string();
    let stderr = tail_bytes(stderr, TAIL_BYTES).to_string();
    with(|c| {
        c.responded = Some(Instant::now());
        c.stdout = stdout;
        c.stderr = stderr;
    });
}

/// A kind that measures its own phases (http) reports them whole.
pub fn note_phases(phases: Value) {
    with(|c| c.phases = Some(phases));
}

impl Capture {
    /// Records output captured outside the python envelope path.
    pub fn set_output(&mut self, stdout: &str, stderr: &str) {
        self.stdout = tail_bytes(stdout, TAIL_BYTES).to_string();
        self.stderr = tail_bytes(stderr, TAIL_BYTES).to_string();
    }

    pub fn stdout(&self) -> &str {
        &self.stdout
    }

    pub fn stderr(&self) -> &str {
        &self.stderr
    }

    /// `queue_ms` is the host's own admission-to-handoff time; the
    /// runner reports its three internal phases; `runner_start_ms` is what is
    /// left of the sandbox round trip once those are taken out (interpreter
    /// boot, the tool module's import, pipe transit) -- so the five add up to
    /// the call's `duration_ms` by construction of what each one measures.
    pub fn timings(&self) -> Value {
        let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
        let queue = self.dispatched.map(|d| ms(d.duration_since(self.t_call))).unwrap_or(0.0);
        if let Some(Value::Object(phases)) = &self.phases {
            let mut out = phases.clone();
            out.insert("queue_ms".to_string(), json!(queue.round() as i64));
            return Value::Object(out);
        }
        match (self.dispatched, self.responded, self.runner) {
            (Some(dispatched), Some(responded), Some((args, tool, ser))) => {
                let round_trip = ms(responded.duration_since(dispatched));
                json!({
                    "queue_ms": queue.round() as i64,
                    "runner_start_ms": (round_trip - args - tool - ser).max(0.0).round() as i64,
                    "args_ms": args.round() as i64,
                    "tool_code_ms": tool.round() as i64,
                    "serialize_ms": ser.round() as i64,
                })
            }
            _ => json!({"queue_ms": queue.round() as i64}),
        }
    }
}

/// Runs `fut` with a fresh call-id slot; returns its output and the id the
/// innermost `call_published_tool` published (`None` if none did).
pub async fn with_call_id_slot<F: Future>(fut: F) -> (F::Output, Option<i64>) {
    let slot = Arc::new(AtomicI64::new(0));
    let out = CALL_ID.scope(slot.clone(), fut).await;
    let id = slot.load(Ordering::SeqCst);
    (out, (id > 0).then_some(id))
}

/// Publishes the `calls.id` just written to the enclosing slot, if any.
pub fn publish_call_id(id: i64) {
    let _ = CALL_ID.try_with(|slot| slot.store(id, Ordering::SeqCst));
}

/// Adds `call_id` (a string, per requirement 1) to a result object, or to the
/// `data` of a structured error; anything else is returned unchanged.
pub fn attach_call_id(result: Result<Value, crate::errors::AppError>, id: i64) -> Result<Value, crate::errors::AppError> {
    use crate::errors::AppError;
    match result {
        Ok(Value::Object(mut map)) => {
            map.insert(crate::envelope::CALL_ID_KEY.to_string(), json!(id.to_string()));
            Ok(Value::Object(map))
        }
        Err(AppError::Structured { code, message, data }) => {
            let data = match data {
                Value::Object(mut map) => {
                    map.insert(crate::envelope::CALL_ID_KEY.to_string(), json!(id.to_string()));
                    Value::Object(map)
                }
                Value::Null => {
                    let mut map = serde_json::Map::new();
                    map.insert(crate::envelope::CALL_ID_KEY.to_string(), json!(id.to_string()));
                    Value::Object(map)
                }
                other => other,
            };
            Err(AppError::Structured { code, message, data })
        }
        other => other,
    }
}

/// Key names whose value is never stored (requirement 3).
fn secret_looking(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    ["key", "token", "secret", "password", "authorization"].iter().any(|p| k.contains(p))
}

/// Strings shorter than this are never used as a redaction needle: a 1-2
/// character "secret" would mangle every unrelated value.
const MIN_NEEDLE: usize = 4;

/// Masks a request before it is stored, and remembers every raw value it
/// masked so the output tails can be scrubbed of the same strings.
pub struct Redactor {
    needles: Vec<(String, String)>,
}

impl Redactor {
    pub fn new(secrets: &HashMap<String, String>) -> Self {
        let needles = secrets
            .iter()
            .filter(|(_, v)| v.len() >= MIN_NEEDLE)
            .map(|(name, v)| (v.clone(), format!("{{{{ secret.{name} }}}}")))
            .collect();
        Self { needles }
    }

    /// Key-pattern masking plus by-value secret replacement over a JSON tree.
    pub fn value(&mut self, v: &Value) -> Value {
        match v {
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| {
                        if secret_looking(k) {
                            self.learn(v);
                            (k.clone(), json!("***"))
                        } else {
                            (k.clone(), self.value(v))
                        }
                    })
                    .collect(),
            ),
            Value::Array(items) => Value::Array(items.iter().map(|i| self.value(i)).collect()),
            Value::String(s) => Value::String(self.text(s)),
            other => other.clone(),
        }
    }

    /// Remember every string under a masked key as something to scrub from
    /// output text.
    fn learn(&mut self, v: &Value) {
        match v {
            Value::String(s) if s.len() >= MIN_NEEDLE => self.needles.push((s.clone(), "***".to_string())),
            Value::Array(items) => items.iter().for_each(|i| self.learn(i)),
            Value::Object(map) => map.values().for_each(|i| self.learn(i)),
            _ => {}
        }
    }

    /// Replaces every known secret value in `s`, longest first so a value
    /// that contains another is fully covered.
    pub fn text(&self, s: &str) -> String {
        let mut order: Vec<&(String, String)> = self.needles.iter().filter(|(n, _)| s.contains(n.as_str())).collect();
        order.sort_by_key(|(n, _)| std::cmp::Reverse(n.len()));
        let mut out = s.to_string();
        for (needle, replacement) in order {
            out = out.replace(needle.as_str(), replacement);
        }
        out
    }
}

/// The failure a trace records: the wire `code`, the message, and for a
/// python exception the `tool.py` line and its source text.
pub fn kind_error_info(err: &crate::kinds::KindError) -> Value {
    use crate::kinds::KindError;
    match err {
        KindError::Structured { code, message, data } => {
            let mut info = json!({"code": code, "message": message});
            for key in ["line", "source_line", "exception_class"] {
                if let Some(v) = data.get(key) {
                    info[key] = v.clone();
                }
            }
            info
        }
        KindError::InvalidSpec(m) => json!({"code": "invalid_spec", "message": m}),
        KindError::InvalidArgs(m) => json!({"code": "args_invalid", "message": m}),
        KindError::Exec(m) => json!({"code": "internal", "message": m}),
    }
}

/// Assembles the row one call stores: sanitized request, redacted and
/// bounded output tails, phase timings, and the parsed failure (if any).
pub fn build_trace(
    secrets: &HashMap<String, String>,
    tool: &str,
    args: &Value,
    capture: Option<&SharedCapture>,
    error: Option<Value>,
) -> crate::db::NewCallTrace {
    let mut redactor = Redactor::new(secrets);
    let mut request = json!({"tool": tool, "args": redactor.value(args)});
    let mut request_json = request.to_string();
    if request_json.len() > REQUEST_BYTES {
        request = json!({"tool": tool, "args": {"_truncated": true, "bytes": request_json.len()}});
        request_json = request.to_string();
    }
    let (stdout, stderr, timings) = match capture {
        Some(cap) => {
            let c = cap.lock().unwrap_or_else(|e| e.into_inner());
            (redactor.text(c.stdout()), redactor.text(c.stderr()), c.timings())
        }
        None => (String::new(), String::new(), json!({"queue_ms": 0})),
    };
    let error_json = error.map(|mut e| {
        for key in ["message", "source_line"] {
            if let Some(Value::String(text)) = e.get(key) {
                e[key] = Value::String(redactor.text(text));
            }
        }
        e.to_string()
    });
    crate::db::NewCallTrace {
        request_json,
        stdout_tail: tail_bytes(&stdout, TAIL_BYTES).to_string(),
        stderr_tail: tail_bytes(&stderr, TAIL_BYTES).to_string(),
        timings_json: timings.to_string(),
        error_json,
    }
}

/// PRD-mcphost-sandbox-timeout-clamp-visible R6: the limits the call ran
/// under ride the untyped `timings_json` blob as `limits`.
pub fn attach_limits(trace: &mut crate::db::NewCallTrace, limits: Option<Value>) {
    let Some(limits) = limits else { return };
    if let Ok(mut timings) = serde_json::from_str::<Value>(&trace.timings_json)
        && let Some(map) = timings.as_object_mut()
    {
        map.insert("limits".to_string(), limits);
        trace.timings_json = timings.to_string();
    }
}

/// Resolves `{{ secret.NAME }}` placeholders in a replay's arguments from the
/// tenant's own secrets, in place. Returns the key of the first value that
/// cannot be restored: one still masked as `***`, or one naming a secret the
/// tenant no longer has.
pub fn unresolved_secret_key(args: &mut Value, secrets: &HashMap<String, String>) -> Option<String> {
    fn walk(v: &mut Value, key: &str, secrets: &HashMap<String, String>) -> Option<String> {
        match v {
            Value::Object(map) => map.iter_mut().find_map(|(k, v)| walk(v, k, secrets)),
            Value::Array(items) => items.iter_mut().find_map(|i| walk(i, key, secrets)),
            Value::String(s) => {
                for (name, value) in secrets {
                    *s = s.replace(&format!("{{{{ secret.{name} }}}}"), value);
                }
                (s == "***" || s.contains("{{ secret.")).then(|| key.to_string())
            }
            _ => None,
        }
    }
    walk(args, "args", secrets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_keeps_the_last_bytes_on_a_char_boundary() {
        let s = format!("{}{}", "a".repeat(10), "é".repeat(3));
        let t = tail_bytes(&s, 5);
        assert!(t.len() <= 5 && s.ends_with(t));
        assert_eq!(tail_bytes("short", 100), "short");
    }

    #[test]
    fn redactor_masks_keys_and_secret_values_and_scrubs_text() {
        let mut secrets = HashMap::new();
        secrets.insert("S".to_string(), "hunter2-value".to_string());
        let mut r = Redactor::new(&secrets);
        let out = r.value(&json!({"api_key": "sk-live-1", "q": "hunter2-value", "n": 3}));
        assert_eq!(out, json!({"api_key": "***", "q": "{{ secret.S }}", "n": 3}));
        assert_eq!(r.text("saw sk-live-1 and hunter2-value"), "saw *** and {{ secret.S }}");
    }
}
