//! contract: reads recipes/*.toml, tests/support/rgpt.rs
//! sylp AC4 (PRD-mcphost-sync-pull-change-detection-live-proof R3): the
//! in-process cap pair `MAX_RECIPE_WALL` (tests/support/rgpt.rs) and
//! `RGPT_MAX_RECIPE_WALL_S` (src/recipes.rs) is derived, not typed: both equal
//! `MAX_POLLED_STEPS x (SCHEDULER_TICK + SYNC_FETCH_ALLOWANCE_S +
//! RETRY_UNTIL_EXPECT_POLL_S) + RECIPE_NON_POLL_STEP_S + RECIPE_WALL_MARGIN_S`,
//! and the data-sync recipe's worst case fits under them.

use mcphost::recipes::{self, Recipe};
use mcphost::triggers::{SCHEDULER_TICK, SYNC_FETCH_ALLOWANCE_S};
use std::path::Path;

fn repo() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn derived() -> u64 {
    recipes::MAX_POLLED_STEPS as u64 * (SCHEDULER_TICK.as_secs() + SYNC_FETCH_ALLOWANCE_S + recipes::RETRY_UNTIL_EXPECT_POLL_S)
        + recipes::RECIPE_NON_POLL_STEP_S
        + recipes::RECIPE_WALL_MARGIN_S
}

#[test]
fn sylp_ac04_validator_cap_equals_the_derivation_from_the_real_constants() {
    assert_eq!(recipes::RGPT_MAX_RECIPE_WALL_S, derived());
    // 2 x (30 + 10 + 5) + 2 + 5 with today's constants.
    assert_eq!(derived(), 97);
}

#[test]
fn sylp_ac04_rgpt_max_recipe_wall_is_the_same_derivation_not_a_literal() {
    let rgpt = std::fs::read_to_string(repo().join("tests/support/rgpt.rs")).expect("read rgpt.rs");
    let decl = rgpt.split("pub const MAX_RECIPE_WALL").nth(1).expect("MAX_RECIPE_WALL declared").split(';').next().unwrap_or("");
    let flat: String = decl.split_whitespace().collect();
    assert!(!flat.contains("from_secs(60)") && !flat.contains("from_secs(97)"), "MAX_RECIPE_WALL must not be a literal: {flat}");
    for term in [
        "mcphost::recipes::MAX_POLLED_STEPS",
        "mcphost::triggers::SCHEDULER_TICK.as_secs()",
        "mcphost::triggers::SYNC_FETCH_ALLOWANCE_S",
        "mcphost::recipes::RETRY_UNTIL_EXPECT_POLL_S",
        "mcphost::recipes::RECIPE_NON_POLL_STEP_S",
        "mcphost::recipes::RECIPE_WALL_MARGIN_S",
    ] {
        assert!(flat.contains(term), "MAX_RECIPE_WALL must be derived from {term}: {flat}");
    }
}

#[test]
fn sylp_ac04_data_sync_worst_case_fits_under_the_cap_and_its_wall_s() {
    let text = std::fs::read_to_string(repo().join("recipes/data-sync.toml")).expect("read data-sync.toml");
    let mut r: Recipe = toml::from_str(&text).expect("parse recipe");
    r.name = "data-sync".to_string();
    let worst = recipes::retry_until_expect_worst_case_s(&r);
    assert!(worst + recipes::RECIPE_WALL_MARGIN_S <= recipes::RGPT_MAX_RECIPE_WALL_S, "worst {worst}");
    assert!(worst <= u64::from(r.budget.wall_s), "worst {worst} vs wall_s {}", r.budget.wall_s);
    recipes::validate(&[r]).expect("data-sync validates against the derived cap");
}
