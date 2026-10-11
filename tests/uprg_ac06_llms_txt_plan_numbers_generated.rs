//! PRD-mcphost-uptime-probe-recipe-green
//! AC6 -- Given `www/llms.txt` section `## Uptime probes with no server`, When
//! `cargo test uprg_` runs, Then every plan number inside the generated spans
//! equals `plans.rs` and a hand edit of one number fails the test naming the
//! span; the section's first call is `host.uptime.create`.

use mcphost::plans::PlanCatalog;
use mcphost::uptime::{expected_spans, render_spans_into, verify_spans};

const LLMS_TXT: &str = include_str!("../www/llms.txt");
const HEADING: &str = "## Uptime probes with no server";

fn section() -> &'static str {
    let start = LLMS_TXT.find(HEADING).expect("llms.txt has the uptime section");
    &LLMS_TXT[start..]
}

#[test]
fn every_span_equals_plans_rs() {
    let catalog = PlanCatalog::default_catalog();
    verify_spans(section(), &catalog).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(expected_spans(&catalog).len(), 6, "free and pro, three numbers each");
}

#[test]
fn committed_section_equals_its_regeneration() {
    let catalog = PlanCatalog::default_catalog();
    assert_eq!(render_spans_into(section(), &catalog), section(), "run `mcphost llms-txt`");
}

#[test]
fn a_hand_edited_number_fails_naming_the_span() {
    let catalog = PlanCatalog::default_catalog();
    let edited = section().replacen(
        "<!-- uprg:pro.schedule_min_interval_s -->60<!--",
        "<!-- uprg:pro.schedule_min_interval_s -->61<!--",
        1,
    );
    assert_ne!(edited, section(), "the edit must land");
    let err = verify_spans(&edited, &catalog).expect_err("a hand edit must fail");
    assert!(err.contains("pro.schedule_min_interval_s"), "{err}");
}

#[test]
fn first_call_in_the_section_is_host_uptime_create() {
    let first_call = section()
        .lines()
        .find_map(|l| l.trim().strip_prefix("host."))
        .expect("the section has a call");
    assert!(first_call.starts_with("uptime.create("), "first call: host.{first_call}");
}
