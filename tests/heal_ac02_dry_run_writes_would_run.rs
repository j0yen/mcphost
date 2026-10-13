//! PRD-mcphost-heal-action-registry-and-ledger AC2 (P0): under
//! `MCPHOST_HEAL_DRY_RUN=1` the closure is not invoked, the row reads
//! `dry_run: 1, outcome: "would_run"` and `/status.json.heal.dry_run` is true.

use crate::common;

use std::sync::atomic::{AtomicUsize, Ordering};

use mcphost::heal::{HealAction, HealOutcome};
use serde_json::json;

#[tokio::test]
async fn dry_run_env_writes_would_run_and_skips_the_closure() {
    let _dry_run = common::EnvGuard::set(mcphost::heal::DRY_RUN_ENV, "1").await;

    let (state, _dir) = common::bare_app_state().await;
    let calls = AtomicUsize::new(0);
    for action in HealAction::ALL {
        let row = mcphost::heal::execute(&state, *action, json!({"reason": "test"}), || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(json!({"before": {"count": 1}, "after": {"count": 0}}))
        })
        .await
        .expect("execute");
        assert_eq!(row.outcome, HealOutcome::WouldRun, "{}", action.as_str());
        assert!(row.dry_run);

        let stored = state.db.find_heal_row(row.id).await.expect("read row").expect("row exists");
        assert!(stored.dry_run);
        assert_eq!(stored.outcome.as_str(), "would_run");
        assert_eq!(stored.trigger, json!({"reason": "test"}));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0, "closure never invoked");

    let status = mcphost::statusfeed::status_json(&state).await.expect("status_json");
    assert_eq!(status["heal"]["dry_run"], json!(true));
    assert_eq!(status["heal"]["last_24h"]["would_run"], json!(HealAction::ALL.len()));
}
