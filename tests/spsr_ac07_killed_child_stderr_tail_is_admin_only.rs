//! PRD-mcphost-sandbox-pool-self-restart AC7 (P1): a killed wedged child's
//! stderr lands in `after.stderr_tails[0]` (at most 2048 bytes) when the row
//! is read through `host.incidents.list`, and `/incidents.json` omits it.

use crate::common;

use std::sync::Arc;
use std::time::Duration;

use mcphost::kinds::python::PythonKind;
use serde_json::{Value, json};

#[tokio::test]
async fn stderr_tail_is_capped_at_2kib_in_the_admin_list_and_absent_from_the_public_feed() {
    let _live = common::EnvGuard::clear(mcphost::heal::DRY_RUN_ENV).await;
    let (mut state, data_dir) = common::bare_app_state().await;
    let python = Arc::new(PythonKind::for_test_with_warm_pool(&data_dir.0, Duration::from_secs(60), 4, 8));
    state.kinds.register(python.clone());

    // 6000 bytes, the last line unmistakable: only the tail may be kept.
    let mut stderr = vec![b'x'; 6000];
    stderr.extend_from_slice(b"\nTRACEBACK: worker stuck in recv()\n");
    let timeout = Duration::from_millis(100);
    let mut wedged = common::WedgedChild::spawn(&stderr);
    let _planted = python.hold_test_flight_with_cancel("7/wedged_tool", timeout, timeout * 4, Some(wedged.slot.clone()));

    let row = mcphost::pool_restart::run(&state, &common::pool_breach()).await.expect("restart").expect("python kind");
    assert_eq!(row.outcome.as_str(), "ok");
    assert!(wedged.is_dead().await);

    let listed = mcphost::heal::incidents_list(&state, &json!({"action": "sandbox_pool_restart"})).await.expect("list");
    let entry = &listed["incidents"][0];
    let after: Value = serde_json::from_str(entry["after_json"].as_str().expect("after_json")).expect("after json");
    let tail = after["stderr_tails"][0].as_str().unwrap_or_else(|| panic!("after: {after}"));
    assert!(tail.len() <= 2048, "tail is {} bytes", tail.len());
    assert!(tail.contains("TRACEBACK: worker stuck in recv()"), "tail keeps the end of stderr");

    let public = mcphost::heal::incidents_json(&state).await.expect("incidents.json");
    let text = public.to_string();
    assert!(text.contains("sandbox_pool_restart"), "the row is in the public feed: {text}");
    assert!(!text.contains("stderr_tails") && !text.contains("TRACEBACK"), "public feed leaks stderr: {text}");
}
