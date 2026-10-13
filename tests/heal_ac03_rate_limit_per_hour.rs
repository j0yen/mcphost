//! PRD-mcphost-heal-action-registry-and-ledger AC3 (P0): with `max_per_hour: 2`
//! and two `ok` rows in the last 3600 s a third call is `rate_limited` and
//! does not run; a fourth call 3601 s after the first `ok` row runs.

use crate::common;

use std::sync::atomic::{AtomicUsize, Ordering};

use mcphost::heal::{HealAction, HealOutcome, execute_at};
use serde_json::json;

#[tokio::test]
async fn third_call_in_the_hour_is_rate_limited_and_fourth_after_the_window_runs() {
    let action = HealAction::DbMaintenance;
    assert_eq!(action.policy().max_per_hour, 2, "this test is written for max_per_hour 2");

    let (state, _dir) = common::bare_app_state().await;
    let calls = AtomicUsize::new(0);
    let run = || async {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"before": {"mb": 5.7}, "after": {"mb": 0}}))
    };
    let t0 = 1_000_000;

    let first = execute_at(&state, action, json!({}), t0, false, run).await.expect("first");
    let second = execute_at(&state, action, json!({}), t0 + 60, false, run).await.expect("second");
    assert_eq!((first.outcome, second.outcome), (HealOutcome::Ok, HealOutcome::Ok));
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    let third = execute_at(&state, action, json!({}), t0 + 120, false, run).await.expect("third");
    assert_eq!(third.outcome, HealOutcome::RateLimited);
    assert_eq!(calls.load(Ordering::SeqCst), 2, "closure not invoked when rate limited");
    let stored = state.db.find_heal_row(third.id).await.expect("read").expect("row");
    assert_eq!(stored.outcome.as_str(), "rate_limited");

    // A rate_limited row never counts toward the limit; the first ok row
    // ages out 3601 s after it was written.
    let fourth = execute_at(&state, action, json!({}), t0 + 3601, false, run).await.expect("fourth");
    assert_eq!(fourth.outcome, HealOutcome::Ok);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}
