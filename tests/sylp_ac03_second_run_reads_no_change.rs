//! contract: reads recipes/data-sync.toml
//! sylp AC3 (PRD-mcphost-sync-pull-change-detection-live-proof R2): a second
//! `host.sync.run_now` and a second polled `host.sync.get` follow the first
//! get; the second get expects ok / no change / 0 rows; the proof body fails
//! when the two gets' run ids match; both polled steps carry `awaits_tick`
//! and share one `retry_s`; `budget.calls` counts every step plus the signup.

use mcphost::recipes::{Recipe, Step};
use serde_json::json;
use std::path::Path;

fn recipe() -> Recipe {
    let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("recipes/data-sync.toml")).expect("read data-sync.toml");
    toml::from_str(&text).expect("parse recipe")
}

fn tools(r: &Recipe) -> Vec<&str> {
    r.steps.iter().map(|s| s.tool.as_deref().unwrap_or("signed_post")).collect()
}

fn value_of(s: &Step, path: &str) -> Option<serde_json::Value> {
    s.expect.iter().find(|e| e.path == path).unwrap_or_else(|| panic!("no expect on {path}")).value.clone()
}

#[test]
fn sylp_ac03_second_run_now_and_second_polled_get_follow_the_first_get() {
    let r = recipe();
    let t = tools(&r);
    let first_get = t.iter().position(|x| *x == "host.sync.get").expect("first get");
    assert_eq!(&t[first_get..first_get + 3], ["host.sync.get", "host.sync.run_now", "host.sync.get"], "{t:?}");
    assert_eq!(t.iter().filter(|x| **x == "host.sync.run_now").count(), 2, "{t:?}");
    let second = &r.steps[first_get + 2];
    assert!(second.retry_until_expect, "the second get is polled");
    assert_eq!(value_of(second, "$.last_run.status"), Some(json!("ok")));
    assert_eq!(value_of(second, "$.last_run.change_detected"), Some(json!(false)));
    assert_eq!(value_of(second, "$.last_run.rows_added"), Some(json!(0)));
}

#[test]
fn sylp_ac03_both_polled_steps_await_the_tick_and_share_one_retry_s() {
    let r = recipe();
    let polled: Vec<&Step> = r.steps.iter().filter(|s| s.retry_until_expect).collect();
    assert_eq!(polled.len(), 2);
    assert!(polled.iter().all(|s| s.awaits_tick), "both polled steps carry awaits_tick");
    assert!(polled[0].retry_s.is_some() && polled[0].retry_s == polled[1].retry_s, "one shared retry_s");
}

#[test]
fn sylp_ac03_budget_calls_counts_every_step_plus_the_signup() {
    let r = recipe();
    assert_eq!(r.budget.calls as usize, r.steps.len() + 1);
    mcphost::recipes::validate(std::slice::from_ref(&{
        let mut r = r.clone();
        r.name = "data-sync".to_string();
        r
    }))
    .expect("data-sync validates");
}

#[test]
fn sylp_ac03_proof_body_fails_when_the_two_run_ids_match() {
    let body = recipe().proof.body;
    assert!(body.contains("FIRST_RUN_ID=\"$GET_RUN_ID\""), "the first get's run_id is captured in a shell variable");
    let line = body
        .lines()
        .find(|l| l.contains("get2_run_id_differs_from_get1"))
        .expect("a check naming the run-id comparison");
    assert!(line.contains("\"$GET_RUN_ID\" != \"$FIRST_RUN_ID\""), "{line}");
    // A failed check bumps FAILURES, which makes the proof exit non-zero.
    assert!(body.contains("FAILURES=$((FAILURES + 1))") && body.contains("echo \"RESULT: FAIL"));
}
