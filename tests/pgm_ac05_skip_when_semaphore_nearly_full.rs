//! PRD-mcphost-performance-guarantees-measured
//! AC5 (P0) -- Given the host-wide semaphore with 1 free permit, When the
//! hourly tick fires, Then no probe call is made and one WARN line names
//! the skip.

use crate::common;
use mcphost::kinds::{KindRegistry, python::PythonKind};
use mcphost::perf::{self, ProbeFuture};
use mcphost::state::AppState;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt::MakeWriter;

static CALLS: AtomicUsize = AtomicUsize::new(0);

fn counting<'a>(_s: &'a AppState, _t: i64, _ns: &'a str) -> ProbeFuture<'a> {
    Box::pin(async {
        CALLS.fetch_add(1, Ordering::SeqCst);
        true
    })
}

#[derive(Clone, Default)]
struct BufWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for BufWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("lock buf").extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for BufWriter {
    type Writer = BufWriter;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test]
async fn pgm_ac05_cycle_skips_with_one_free_permit_and_runs_with_two() {
    let (mut state, _dir) = common::bare_app_state().await;
    let envs = common::TempDataDir::new();
    let py = Arc::new(PythonKind::for_test_with_concurrency(&envs.0, 3));
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(py.clone());
    state.kinds = kinds;
    let probes: perf::Probes = vec![("echo", counting)];

    // 3 permits, 2 held -> 1 free.
    let held = py.hold_admission_for_test(1, 4, 2);
    let buf = BufWriter::default();
    let subscriber = tracing_subscriber::fmt().with_writer(buf.clone()).with_ansi(false).finish();
    // Thread-local default: this test runs on a current-thread runtime, so
    // the `.await` below stays on the thread the guard covers.
    let guard = tracing::subscriber::set_default(subscriber);
    let ran = perf::run_cycle(&state, &probes).await.expect("cycle");
    drop(guard);
    assert!(!ran, "cycle must report it skipped");
    assert_eq!(CALLS.load(Ordering::SeqCst), 0, "no probe call with 1 free permit");
    let log = String::from_utf8(buf.0.lock().expect("buf").clone()).expect("utf8");
    let warns: Vec<&str> = log.lines().filter(|l| l.contains("WARN") && l.contains("perf probe cycle skipped")).collect();
    assert_eq!(warns.len(), 1, "exactly one WARN naming the skip: {log}");

    // Releasing one permit leaves 2 free: the cycle runs.
    drop(held);
    assert!(perf::run_cycle(&state, &probes).await.expect("cycle"));
    assert_eq!(CALLS.load(Ordering::SeqCst), perf::SAMPLES_PER_CYCLE * perf::PLANS.len());
}
