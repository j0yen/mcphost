//! PRD-mcphost-heal-action-registry-and-ledger AC3 (P0): with `max_per_hour: N`
//! and N `ok` rows in the last 3600 s the next call is `rate_limited` and
//! does not run; a call 3601 s after the first `ok` row runs.

use crate::common;

use std::sync::atomic::{AtomicUsize, Ordering};

use mcphost::heal::{HealAction, HealOutcome, execute_at};
use serde_json::json;

#[tokio::test]
async fn third_call_in_the_hour_is_rate_limited_and_fourth_after_the_window_runs() {
    let action = HealAction::SandboxPoolRestart;
    let max = action.policy().max_per_hour;
    assert!(max >= 2, "this test needs a multi-run policy");

    let (state, _dir) = common::bare_app_state().await;
    let calls = AtomicUsize::new(0);
    let run = || async {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"before": {"mb": 5.7}, "after": {"mb": 0}}))
    };
    let t0 = 1_000_000;

    for i in 0..i64::from(max) {
        let row = execute_at(&state, action, json!({}), t0 + i * 60, false, run).await.expect("within limit");
        assert_eq!(row.outcome, HealOutcome::Ok);
    }
    assert_eq!(calls.load(Ordering::SeqCst), max as usize);

    let over = execute_at(&state, action, json!({}), t0 + 600, false, run).await.expect("over limit");
    assert_eq!(over.outcome, HealOutcome::RateLimited);
    assert_eq!(calls.load(Ordering::SeqCst), max as usize, "closure not invoked when rate limited");
    let stored = state.db.find_heal_row(over.id).await.expect("read").expect("row");
    assert_eq!(stored.outcome.as_str(), "rate_limited");

    // A rate_limited row never counts toward the limit; the first ok row
    // ages out 3601 s after it was written.
    let next = execute_at(&state, action, json!({}), t0 + 3601, false, run).await.expect("after window");
    assert_eq!(next.outcome, HealOutcome::Ok);
    assert_eq!(calls.load(Ordering::SeqCst), max as usize + 1);
}
