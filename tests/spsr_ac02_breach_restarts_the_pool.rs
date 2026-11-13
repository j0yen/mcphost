//! PRD-mcphost-sandbox-pool-self-restart-v2 AC2: a `SignalEvent::Breach` for
//! `SandboxPool` with `MCPHOST_HEAL_DRY_RUN` unset makes the subscriber run
//! the executor: the stalled flight's process group is dead,
//! `/status.json.sandbox_pool` reads `ready` with 0 wedged, and the incidents
//! row is `sandbox_pool_restart`, outcome `ok`, `before.wedged` 1 and
//! `after.wedged` 0.

use crate::common::EnvGuard;
use crate::spsr;

use serde_json::json;

#[tokio::test]
async fn spsr_ac02() {
    if mcphost::sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", mcphost::sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let _dry = EnvGuard::clear(mcphost::heal::DRY_RUN_ENV).await;
    let (state, python, _dir) = spsr::app(5).await;
    let wedge = spsr::wedge(&python, "stalled_tool").await;
    assert!(spsr::group_alive(wedge.pgid), "the stalled child is alive before the restart");

    let _subscriber = mcphost::pool_restart::spawn_subscriber(state.clone());
    spsr::breach(&state);
    let row = spsr::wait_row(&state).await;

    assert_eq!(row.outcome.as_str(), "ok", "row: {row:?}");
    assert_eq!(row.before["wedged"], 1, "row: {row:?}");
    assert_eq!(row.after["wedged"], 0, "row: {row:?}");
    assert!(!spsr::group_alive(wedge.pgid), "the stalled flight's process group is dead");
    assert!(spsr::all_gone(&wedge.tree).await, "the stalled child's interpreter died with it");
    let err = wedge.call.await.unwrap().expect_err("the stalled call fails");
    assert!(spsr::is_tool_timeout(&err), "got {err:?}");

    let status = mcphost::statusfeed::status_json(&state).await.unwrap();
    let pool = &status["sandbox_pool"];
    assert_eq!(pool["state"], "ready", "{pool}");
    assert_eq!(pool["wedged"], 0, "{pool}");
    assert_eq!(pool["last_restart"]["heal_action_id"], row.id, "{pool}");

    let listed = mcphost::heal::incidents_list(&state, &json!({"action": "sandbox_pool_restart"})).await.unwrap();
    let incident = &listed["incidents"][0];
    assert_eq!(incident["action"], "sandbox_pool_restart");
    assert_eq!(incident["outcome"], "ok");
}
