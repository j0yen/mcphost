//! PRD-mcphost-table-import-url-live-proof
//! AC5 (P1) -- Given the hand-written `## Give Claude a database in one
//! minute` section of `www/llms.txt` (the generated `recipe:database-in-a-minute`
//! block stays generator-owned), When an agent reads it, Then the one-call form
//! (`host_table_create` with `source_url`
//! `https://mcphost.dev/examples/sample-1000.csv`) comes before the
//! batched-append step, and the AC 2 drift test covers that URL.

use crate::tblurlp_ac02_example_url_route_drift::{advertised_in_tree, advertised_paths};
use std::collections::BTreeSet;

const HEADING: &str = "## Give Claude a database in one minute";
const FIXTURE_URL: &str = "https://mcphost.dev/examples/sample-1000.csv";

/// The hand-written section: from its own heading to the next `## ` heading or
/// recipe marker. The generated block's heading is `## Recipe calls: ...`, so
/// the exact-line match cannot land in it.
fn hand_written_section() -> String {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/www/llms.txt")).expect("read www/llms.txt");
    let mut section = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line == HEADING {
            inside = true;
            continue;
        }
        if inside && (line.starts_with("## ") || line.starts_with("<!-- recipe:")) {
            break;
        }
        if inside {
            section.push(line);
        }
    }
    assert!(inside, "`{HEADING}` is a heading in www/llms.txt");
    section.join("\n")
}

#[test]
fn tblurlp_ac05_one_call_form_comes_before_the_batched_append_step() {
    let section = hand_written_section();
    let one_call = section.find("host_table_create").expect("the one-call `host_table_create` form");
    let url = section.find(FIXTURE_URL).expect("the fixture URL");
    let append = section.find("host.table.append").expect("the batched-append step");
    assert!(one_call < append, "`host_table_create` precedes the batched-append step");
    assert!(url < append, "the fixture URL precedes the batched-append step");
    let call = &section[one_call..url + FIXTURE_URL.len()];
    assert!(call.contains("source_url"), "the one-call form passes `source_url`: {call}");
}

#[test]
fn tblurlp_ac05_the_recipe_url_is_covered_by_the_ac2_drift_test() {
    let section = hand_written_section();
    assert!(advertised_paths(&section).contains("/examples/sample-1000.csv"), "the drift scan sees the section's URL");
    let scanned: BTreeSet<_> = advertised_in_tree().into_iter().map(|(file, path)| (file.display().to_string(), path)).collect();
    assert!(
        scanned.contains(&("www/llms.txt".to_string(), "/examples/sample-1000.csv".to_string())),
        "the AC 2 tree scan reports www/llms.txt advertising the fixture URL: {scanned:?}"
    );
}
