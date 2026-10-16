//! AC2 -- Given a running server with 20 in-flight python calls of 2 s each
//! (20 = the python admission cap) and `MCPHOST_DRAIN_S=20`, When a real
//! SIGTERM arrives, Then all 20 return their real results, the process exits
//! 0 within 5 s of the signal, and the `restarts` row reads
//! `in_flight_at_signal: 20, finished: 20, dropped: 0`.

use crate::common;
use crate::gdrain;
use crate::serve;

use std::time::Duration;

#[tokio::test]
async fn gdrain_ac02() {
    if mcphost::sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", mcphost::sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let mut fx = gdrain::start("ac02", 20, 5).await;
    let calls = fx.fire(20, 2).await;

    let server: &mut serve::ServeChild = &mut fx.server;
    server.terminate();

    for (i, call) in calls.into_iter().enumerate() {
        let value = call
            .await
            .expect("call task")
            .unwrap_or_else(|e| panic!("call {i} must return its real result: {} {}", e.code, e.message));
        assert_eq!(common::extract_structured(&value)["ok"], serde_json::json!(true), "call {i}: {value}");
    }

    let status = server
        .wait_exit(Duration::from_secs(5))
        .expect("process must exit within 5 s of the signal");
    assert_eq!(status.code(), Some(0), "exit status {status:?}");

    let (in_flight, finished, dropped, _drain_ms, bound_s) = fx.restart_row();
    assert_eq!((in_flight, finished, dropped), (20, 20, 0));
    assert_eq!(bound_s, 20);
}
