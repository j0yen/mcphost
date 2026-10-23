//! contract: reads recipes/*.toml
//! sylp AC1 (PRD-mcphost-sync-pull-change-detection-live-proof R4): the recipe
//! validator refuses an `awaits_tick` step whose `retry_s` is under one
//! scheduler tick plus the fetch allowance, refuses more than `MAX_POLLED_STEPS`
//! polls and a `budget.wall_s` under the worst case, and accepts two
//! `awaits_tick` polls of exactly that `retry_s`.

use mcphost::recipes::{self, Recipe};
use mcphost::triggers::{SCHEDULER_TICK, SYNC_FETCH_ALLOWANCE_S};
use std::path::Path;

fn repo() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn base() -> Recipe {
    let text = std::fs::read_to_string(repo().join("recipes/data-sync.toml")).expect("read data-sync.toml");
    let mut r: Recipe = toml::from_str(&text).expect("parse recipe");
    r.name = "data-sync".to_string();
    r
}

/// data-sync's polled get duplicated until it has `polls` polled steps, every
/// one `awaits_tick` with `retry_s`.
fn with_polls(polls: usize, retry_s: u64, wall_s: u32) -> Recipe {
    let mut r = base();
    let get = r.steps.iter().find(|s| s.retry_until_expect).expect("a polled step").clone();
    r.steps.retain(|s| !s.retry_until_expect);
    for _ in 0..polls {
        let mut s = get.clone();
        s.awaits_tick = true;
        s.retry_s = Some(retry_s);
        r.steps.insert(2, s);
    }
    r.budget.wall_s = wall_s;
    r
}

fn min_retry() -> u64 {
    SCHEDULER_TICK.as_secs() + SYNC_FETCH_ALLOWANCE_S
}

#[test]
fn sylp_ac01_min_retry_is_tick_plus_fetch_allowance() {
    assert_eq!(recipes::awaits_tick_min_retry_s(), min_retry());
    assert_eq!(SYNC_FETCH_ALLOWANCE_S, 10);
    assert_eq!(recipes::MAX_POLLED_STEPS, 2);
}

#[test]
fn sylp_ac01_two_awaits_tick_polls_of_exactly_the_minimum_are_accepted() {
    let r = with_polls(2, min_retry(), 95);
    recipes::validate(&[r]).expect("two awaits_tick polls at retry_s = tick + allowance must validate");
}

#[test]
fn sylp_ac01_awaits_tick_retry_s_below_the_minimum_is_refused() {
    let r = with_polls(2, min_retry() - 1, 95);
    let err = recipes::validate(&[r]).expect_err("retry_s under a tick plus the fetch must be refused").to_string();
    assert!(err.contains("awaits_tick") && err.contains("SCHEDULER_TICK"), "{err}");

    // Without retry_s at all it is refused too, not silently skipped.
    let mut r = with_polls(1, min_retry(), 95);
    for s in r.steps.iter_mut().filter(|s| s.awaits_tick) {
        s.retry_s = None;
        s.retry_until_expect = false;
    }
    let err = recipes::validate(&[r]).expect_err("awaits_tick without retry_s must be refused").to_string();
    assert!(err.contains("awaits_tick"), "{err}");
}

#[test]
fn sylp_ac01_more_than_max_polled_steps_is_refused() {
    let r = with_polls(recipes::MAX_POLLED_STEPS + 1, min_retry(), 200);
    let err = recipes::validate(&[r]).expect_err("three polls must be refused").to_string();
    assert!(err.contains("MAX_POLLED_STEPS"), "{err}");
}

#[test]
fn sylp_ac01_wall_s_below_the_worst_case_is_refused() {
    let r = with_polls(2, min_retry(), 95);
    let worst = recipes::retry_until_expect_worst_case_s(&r);
    assert_eq!(worst, 2 * (min_retry() + recipes::RETRY_UNTIL_EXPECT_POLL_S) + recipes::RECIPE_NON_POLL_STEP_S);
    let mut low = r.clone();
    low.budget.wall_s = (worst - 1) as u32;
    let err = recipes::validate(&[low]).expect_err("wall_s under the worst case must be refused").to_string();
    assert!(err.contains("budget.wall_s"), "{err}");
    let mut exact = r;
    exact.budget.wall_s = worst as u32;
    recipes::validate(&[exact]).expect("wall_s equal to the worst case validates");
}

#[test]
fn sylp_ac01_every_recipe_under_recipes_validates() {
    let all = recipes::load_all(repo()).expect("load recipes");
    assert!(!all.is_empty());
    recipes::validate(&all).expect("every committed recipe validates");
}
