//! PRD-mcphost-sync-pull-change-detection-live-proof AC 1 -- the recipe
//! validator refuses an `awaits_tick` poll shorter than one scheduler tick plus
//! the fetch allowance, more than `MAX_POLLED_STEPS` polls, and a `wall_s`
//! below the polling worst case; two polls of exactly the floor are accepted.

use mcphost::recipes::{self, Recipe, Step};
use mcphost::triggers::{SCHEDULER_TICK, SYNC_FETCH_ALLOWANCE_S};
use std::path::Path;

fn floor() -> u64 {
    SCHEDULER_TICK.as_secs() + SYNC_FETCH_ALLOWANCE_S
}

fn data_sync() -> Recipe {
    let all = recipes::load_all(Path::new(env!("CARGO_MANIFEST_DIR"))).expect("load recipes");
    all.into_iter().find(|r| r.name == "data-sync").expect("recipes/data-sync.toml")
}

fn poll_step(r: &Recipe) -> Step {
    r.steps.iter().find(|s| s.retry_until_expect).cloned().expect("data-sync has a polled step")
}

/// data-sync with exactly `n` polled steps, all `awaits_tick` at `retry_s`.
fn with_polls(n: usize, retry_s: u64) -> Recipe {
    let mut r = data_sync();
    let template = poll_step(&r);
    r.steps.retain(|s| !s.retry_until_expect);
    for _ in 0..n {
        let mut s = template.clone();
        s.awaits_tick = true;
        s.retry_s = Some(retry_s);
        r.steps.insert(1, s);
    }
    r
}

fn refusal(r: Recipe) -> String {
    recipes::validate(&[r]).expect_err("the validator must refuse this recipe").to_string()
}

#[test]
fn sylp_ac01_every_committed_recipe_validates() {
    let all = recipes::load_all(Path::new(env!("CARGO_MANIFEST_DIR"))).expect("load recipes");
    assert!(!all.is_empty());
    recipes::validate(&all).expect("every recipes/*.toml validates");
}

#[test]
fn sylp_ac01_awaits_tick_below_tick_plus_fetch_allowance_is_refused() {
    assert_eq!(floor(), 40, "SCHEDULER_TICK 30 s + SYNC_FETCH_ALLOWANCE_S 10 s");
    let err = refusal(with_polls(1, floor() - 1));
    assert!(err.contains("awaits_tick") && err.contains("retry_s"), "{err}");
}

#[test]
fn sylp_ac01_more_than_max_polled_steps_is_refused() {
    let err = refusal(with_polls(recipes::MAX_POLLED_STEPS as usize + 1, floor()));
    assert!(err.contains("polled steps"), "{err}");
}

#[test]
fn sylp_ac01_wall_s_below_the_worst_case_is_refused() {
    let mut r = with_polls(2, floor());
    let worst = recipes::retry_until_expect_worst_case_s(&r);
    assert_eq!(worst, 2 * (floor() + recipes::RETRY_UNTIL_EXPECT_POLL_S) + recipes::RECIPE_NON_POLL_STEP_S);
    r.budget.wall_s = (worst - 1) as u32;
    let err = refusal(r);
    assert!(err.contains("budget.wall_s"), "{err}");
}

#[test]
fn sylp_ac01_two_awaits_tick_polls_of_exactly_the_floor_are_accepted() {
    let mut r = with_polls(2, floor());
    r.budget.wall_s = recipes::retry_until_expect_worst_case_s(&r) as u32;
    recipes::validate(&[r]).expect("two awaits_tick polls at the floor fit the derived cap");
}

#[test]
fn sylp_ac01_awaits_tick_needs_retry_until_expect() {
    let mut r = data_sync();
    let s = r.steps.iter_mut().find(|s| s.tool.as_deref() == Some("host.sync.list")).expect("list step");
    s.awaits_tick = true;
    let err = refusal(r);
    assert!(err.contains("awaits_tick") && err.contains("retry_until_expect"), "{err}");
}
