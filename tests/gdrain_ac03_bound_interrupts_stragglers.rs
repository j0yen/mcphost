//! AC3 -- Given 5 in-flight calls of 30 s each and `MCPHOST_DRAIN_S=2`, When
//! SIGTERM arrives, Then each caller receives code `interrupted` with
//! `data.restart_id`, their `calls` rows read `error_class = 'interrupted'`,
//! and the row reads `finished: 0, dropped: 5, drain_ms` within 2 000-2 500.

use crate::gdrain;
use crate::serve;

use std::time::Duration;

#[tokio::test]
async fn gdrain_ac03() {
    if mcphost::sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", mcphost::sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let mut fx = gdrain::start("ac03", 2, 2).await;
    let calls = fx.fire(5, 30).await;

    let server: &mut serve::ServeChild = &mut fx.server;
    server.terminate();

    let mut restart_ids = Vec::new();
    for (i, call) in calls.into_iter().enumerate() {
        let err = call.await.expect("call task").expect_err("a 30 s call must not finish inside a 2 s bound");
        assert_eq!(err.error_code.as_deref(), Some("interrupted"), "call {i}: {err:?}");
        let id = err.data["restart_id"].as_str().unwrap_or_else(|| panic!("call {i}: no data.restart_id in {err:?}"));
        restart_ids.push(id.to_string());
    }

    let status = server.wait_exit(Duration::from_secs(10)).expect("process exits");
    assert_eq!(status.code(), Some(0), "exit status {status:?}");

    let (in_flight, finished, dropped, drain_ms, bound_s) = fx.restart_row();
    assert_eq!((in_flight, finished, dropped, bound_s), (5, 0, 5, 2));
    assert!((2_000..=2_500).contains(&drain_ms), "drain_ms {drain_ms}");
    assert!(restart_ids.iter().all(|id| *id == fx.restart_id()), "{restart_ids:?}");
    assert_eq!(fx.calls_with_class("interrupted"), 5);
}
