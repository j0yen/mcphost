//! PRD-mcphost-docs-qa-recipe
//! AC3 -- Given `www/llms.txt`, When the "Docs Q&A in a minute" section is
//! followed literally by the test (each call parsed from the doc and
//! executed), Then the sequence succeeds -- the doc and the script agree.
//!
//! The section's five numbered steps each lead with exactly one inline-code
//! call (`signup(...)`, `host.docs.put(...)`, `host.docs.status()`,
//! `host.tool.publish(...)`, `<namespace>.ask_docs(...)`); this test parses
//! that ordered call sequence straight out of the doc's own numbered list
//! (a hand-rolled scan, same "no need for a whole parser crate for one
//! self-owned fixture" precedent `mcphost_uptime_probes_ac07_*`'s
//! `eval_call_check` mini-grammar already uses) and asserts it is exactly
//! the ordered, deduplicated call sequence a real `docs-qa.sh` run against
//! the in-process test host actually made (recorded in its own receipt's
//! `calls` field) -- so the doc can never silently drift from what the
//! script does.
// PRD-mcphost-tool-naming-convention-and-aliases: updated to the canonical name -- docs/www/llms.txt now read host.<family>.<verb>, not the old underscore form this test used to parse/compare against.

use crate::common;
use crate::docs_qa;

use common::{TempDataDir, TestServer, python_kind_registry};

const LLMS_TXT: &str = include_str!("../www/llms.txt");
const SECTION_HEADING: &str = "## Docs Q&A in a minute";

fn section_text() -> &'static str {
    let start = LLMS_TXT
        .find(SECTION_HEADING)
        .expect("www/llms.txt must have a '## Docs Q&A in a minute' section");
    let rest = &LLMS_TXT[start..];
    let end = rest[SECTION_HEADING.len()..]
        .find("\n## ")
        .map(|off| SECTION_HEADING.len() + off)
        .unwrap_or(rest.len());
    &rest[..end]
}

/// A call ending in `.ask_docs` (the doc's `<namespace>.ask_docs`, or the
/// receipt's own `<real-namespace>.ask_docs`) normalizes to the bare tool
/// name -- the doc can't name a real tenant's namespace ahead of time, so
/// this is the one place doc and receipt necessarily differ verbatim.
fn normalize_call(token: &str) -> String {
    if token == "ask_docs" || token.ends_with(".ask_docs") {
        "ask_docs".to_string()
    } else {
        token.to_string()
    }
}

/// Scans the section's numbered list (`"N. `call(...)`"` lines) for the
/// first inline-code call each leads with, in order.
fn ordered_calls_from_section(section: &str) -> Vec<String> {
    let mut calls = Vec::new();
    for line in section.lines() {
        let trimmed = line.trim_start();
        let after_digits = trimmed.trim_start_matches(|c: char| c.is_ascii_digit());
        if after_digits.len() == trimmed.len() || !after_digits.starts_with(". ") {
            continue; // not a "N. " numbered step line
        }
        let Some(tick) = trimmed.find('`') else { continue };
        let after_tick = &trimmed[tick + 1..];
        let Some(paren) = after_tick.find('(') else { continue };
        calls.push(normalize_call(&after_tick[..paren]));
    }
    calls
}

#[tokio::test]
async fn doc_and_script_agree_on_the_call_sequence() {
    let parsed_calls = ordered_calls_from_section(section_text());
    assert_eq!(
        parsed_calls,
        vec!["signup", "host.docs.put", "host.docs.status", "host.tool.publish", "ask_docs"],
        "the doc's five numbered steps must each lead with exactly one of these calls, in order"
    );

    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let endpoint = format!("{}/mcp", server.base_url);
    let receipt_dir = docs_qa::scratch_receipt_dir("ac03");

    let run = docs_qa::run_docs_qa(
        &endpoint,
        &["--receipt-dir", receipt_dir.to_str().expect("utf8 path")],
        &[],
    )
    .await;
    assert!(run.success, "docs-qa.sh must exit 0\nstdout:\n{}\nstderr:\n{}", run.stdout, run.stderr);

    let receipt = run.receipt();
    let actual_calls: Vec<String> = receipt["calls"]
        .as_array()
        .expect("receipt.calls array")
        .iter()
        .map(|v| normalize_call(v.as_str().expect("call name is a string")))
        .collect();

    assert_eq!(
        actual_calls, parsed_calls,
        "the script's own ordered, deduplicated call sequence must match the doc: receipt={receipt}"
    );

    std::fs::remove_dir_all(&receipt_dir).ok();
}
