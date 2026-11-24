//! PRD-mcphost-sandbox-pool-self-restart-v2 AC5: with `MCPHOST_HEAL_DRY_RUN=1`
//! a breach writes a `would_run` row with `trigger.wedged` 1, the stalled
//! child stays alive, and `/status.json.sandbox_pool.state` stays `ready`.

use crate::common::EnvGuard;
use crate::spsr;

#[tokio::test]
async fn spsr_ac05() {
    if mcphost::sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", mcphost::sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let _dry = EnvGuard::set(mcphost::heal::DRY_RUN_ENV, "1").await;
    let (state, python, _dir) = spsr::app(5).await;
    let wedge = spsr::wedge(&python, "stalled_tool").await;

    let _subscriber = mcphost::pool_restart::spawn_subscriber(state.clone());
    spsr::breach(&state);
    let row = spsr::wait_row(&state).await;

    assert_eq!(row.outcome.as_str(), "would_run", "row: {row:?}");
    assert!(row.dry_run);
    assert_eq!(row.trigger["wedged"], 1, "row: {row:?}");
    assert!(spsr::group_alive(wedge.pgid), "the stalled child is still alive");
    assert!(!wedge.call.is_finished(), "the stalled call is still blocked");

    let status = mcphost::statusfeed::status_json(&state).await.unwrap();
    assert_eq!(status["sandbox_pool"]["state"], "ready", "{}", status["sandbox_pool"]);
    assert_eq!(status["sandbox_pool"]["wedged"], 1, "{}", status["sandbox_pool"]);

    wedge.call.abort();
}
