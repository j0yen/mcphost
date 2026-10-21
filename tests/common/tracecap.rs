//! Request-log / audit-line capture for tests: ONE process-global tracing
//! subscriber per test binary, installed once, never changed again.
//!
//! Why this exists (mcphost flake class `sessbind_ac10` / `incbanb_ac03` /
//! `gsigb_ac03` / `pgm_ac05` / `oauthmeta_ac04` / `busyaudit`+`capture_tracing`):
//! those tests used to capture a log line with a thread-local
//! `tracing::subscriber::set_default` (or `with_default`) plus
//! `tracing::callsite::rebuild_interest_cache()`. In a consolidated
//! suite binary several such tests run concurrently, and `tracing`'s
//! per-callsite `Interest` cache is process-global: while one test's
//! scoped dispatcher is being installed or torn down, another thread can
//! register (or a rebuild can recompute) the very callsite under test as
//! "never interested", and the event is then DROPPED before any subscriber
//! is asked about it. The line is MISSING, not late -- no wait, retry or
//! longer timeout can bring it back (CI run 39681889523, shard 5/5,
//! `sessbind_ac10` read `left: None, right: Some(401)`).
//!
//! The fix is to never touch the dispatcher after process start. The first
//! [`capture_json`]/[`capture_text`] call installs a global `Registry` with
//! two `fmt` layers (JSON and plain text). Their per-layer filter is a
//! `filter_fn` (a dynamic filter: its callsite interest is "sometimes", so
//! it is consulted per event and the cache is never poisoned to "never").
//! Each [`Capture`] registers a sink on the CALLING THREAD's stack; an event
//! is formatted only when its emitting thread has a capture open, and is
//! written only into that thread's innermost capture. That is exactly the
//! isolation `set_default` gave (a `#[tokio::test]` current-thread runtime
//! keeps the whole round trip on the test thread), so concurrent tests never
//! cross-read, and no test needs a marker, a lock, a warm-up request or a
//! retry loop.
//!
//! Max level is INFO (the `tracing_subscriber::fmt()` default the old
//! captures used).
//!
//! Binaries that cannot share a global subscriber (they call
//! `tracing::subscriber::set_global_default` themselves) are routed to their
//! own singleton suite by `scripts/gen-test-suites.sh` and never use this
//! module. `tests/tracecap_ac01_no_bare_tracing_default_in_tests.rs` fails
//! the build if any other test file reaches for the scoped-default pattern.

#![allow(dead_code)]

use std::cell::RefCell;
use std::io;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, OnceLock};

use tracing::Level;
use tracing_subscriber::Layer;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;

/// Which of the two installed formatters a [`Capture`] reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// `fmt().json().with_target(false)` -- one JSON object per line.
    Json,
    /// `fmt().with_ansi(false)` -- the plain one-line text format.
    Text,
}

struct Sink {
    format: Format,
    buf: Mutex<Vec<u8>>,
}

thread_local! {
    /// The captures open on this thread, innermost last.
    static STACK: RefCell<Vec<Arc<Sink>>> = const { RefCell::new(Vec::new()) };
}

fn wants(format: Format, level: &Level) -> bool {
    // `Level` orders by verbosity: ERROR < WARN < INFO < DEBUG < TRACE.
    *level <= Level::INFO
        && STACK
            .try_with(|s| s.try_borrow().ok().and_then(|s| s.last().map(|k| k.format)))
            .ok()
            .flatten()
            == Some(format)
}

#[derive(Clone, Copy)]
struct HubWriter;

struct SinkWriter(Option<Arc<Sink>>);

impl io::Write for SinkWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Some(sink) = &self.0 {
            sink.buf.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(bytes);
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for HubWriter {
    type Writer = SinkWriter;
    fn make_writer(&'a self) -> SinkWriter {
        SinkWriter(STACK.try_with(|s| s.try_borrow().ok().and_then(|s| s.last().cloned())).ok().flatten())
    }
}

fn install() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let json = tracing_subscriber::fmt::layer()
            .json()
            .with_target(false)
            .with_writer(HubWriter)
            .with_filter(filter_fn(|m| wants(Format::Json, m.level())));
        let text = tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(HubWriter)
            .with_filter(filter_fn(|m| wants(Format::Text, m.level())));
        tracing::subscriber::set_global_default(tracing_subscriber::registry().with(json).with(text)).expect(
            "tracecap: a global tracing subscriber is already installed in this test binary; \
             a test file that calls set_global_default belongs in its own singleton suite \
             (scripts/gen-test-suites.sh) and must not use tests/common/tracecap.rs",
        );
    });
}

/// Events emitted on the creating thread while this value is alive. Not
/// `Send`: the capture is keyed to the thread that created it.
pub struct Capture {
    sink: Arc<Sink>,
    _thread_bound: PhantomData<*const ()>,
}

fn open(format: Format) -> Capture {
    install();
    let sink = Arc::new(Sink { format, buf: Mutex::new(Vec::new()) });
    STACK.with(|s| s.borrow_mut().push(sink.clone()));
    Capture { sink, _thread_bound: PhantomData }
}

/// Capture this thread's events as JSON lines (`fields.<name>`, `level`, ...).
pub fn capture_json() -> Capture {
    open(Format::Json)
}

/// Capture this thread's events as plain text lines.
pub fn capture_text() -> Capture {
    open(Format::Text)
}

impl Capture {
    /// Everything captured so far.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.sink.buf.lock().unwrap_or_else(|e| e.into_inner())).into_owned()
    }

    /// Every captured line that parses as a JSON object ([`capture_json`]).
    pub fn json_lines(&self) -> Vec<serde_json::Value> {
        self.text().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
    }

    /// Stop capturing and return everything captured.
    pub fn finish(self) -> String {
        self.text()
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = STACK.try_with(|s| {
            let mut s = s.borrow_mut();
            if let Some(i) = s.iter().rposition(|k| Arc::ptr_eq(k, &self.sink)) {
                s.remove(i);
            }
        });
    }
}
