//! PRD-mcphost-checkcompat-port-race AC2 (P0) — Given a foreign HTTP
//! server already answering 200 on the port check-compat chose (test
//! binds its own listener with `SO_REUSEADDR` on the same port before the
//! child starts, or injects a fake responder), When `wait_ready` polls,
//! Then it does not report ready and the log contains `foreign server on
//! port`.
//!
//! With requirement 1's inherited-fd design, `check_compat` binds the
//! port itself before any child exists, so nothing else can ever hold
//! that exact port first -- the true SO_REUSEADDR race the PRD's problem
//! statement describes can no longer reach `wait_ready` at all. This test
//! takes the PRD's offered alternative ("or injects a fake responder"):
//! it drives `wait_ready` directly (via `compat_check::test_support`,
//! gated behind the same `test-support` feature `billing::FakeBillingClient`
//! uses) against a `wiremock` server that answers `/healthz` with 200 and
//! no `X-Mcphost-Compat-Token` header at all -- exactly what "some other
//! process is on this port" looks like from `wait_ready`'s point of view,
//! per requirement 2.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tracing_subscriber::fmt::MakeWriter;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Captures this process's own tracing output -- same pattern as
/// `tests/autherr_ac6_warn_journal_no_key.rs` -- since a full journald
/// capture isn't practical from an integration test.
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
async fn foreign_server_without_token_never_reports_ready() {
    let buf = BufWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_writer(buf.clone())
        .with_target(false)
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .expect("install the test's global tracing subscriber");

    // The foreign server: answers 200 on /healthz, knows nothing about
    // any compat token.
    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/healthz"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;
    let port = mock_server.address().port();

    // A real, still-running process standing in for "the previous
    // release" -- it never touches the mock server's port, but it must
    // stay alive so `wait_ready`'s `Child::try_wait` keeps polling
    // instead of reporting the child exited (that's AC1's scenario, not
    // this one).
    let mut child = tokio::process::Command::new("sleep")
        .arg("30")
        .kill_on_drop(true)
        .spawn()
        .expect("spawn dummy previous-release stand-in");

    let expected_token = "expected-token-the-foreign-server-does-not-have";
    let result = tokio::time::timeout(
        Duration::from_millis(800),
        mcphost::compat_check::test_support::wait_ready(
            &mock_server.uri(),
            port,
            expected_token,
            &mut child,
        ),
    )
    .await;

    assert!(
        result.is_err(),
        "wait_ready must not report ready against a server that never carries our token \
         (it should still be polling when the test's own short timeout elapses)"
    );

    let _ = child.start_kill();
    let _ = child.wait().await;

    let captured =
        String::from_utf8(buf.0.lock().expect("lock buf").clone()).expect("captured log is UTF-8");
    assert!(
        captured.contains("foreign server on port"),
        "log should record the foreign-server detection, got: {captured}"
    );
}
