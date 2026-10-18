//! contract: reads registry handler::host_tool_descriptors
//! PRD-mcphost-feedback-tool-and-ledger AC6 — Given `FeedbackKind::ALL`, When
//! `fbtl_kinds_cannot_drift` runs, Then the `host.feedback.send` schema enum,
//! the llms.txt kinds line, and `ALL` are identical, and removing one kind
//! from any of the three fails the test.

use mcphost::feedback::FeedbackKind;
use mcphost::handler::host_tool_descriptors;
use mcphost::kinds::KindRegistry;

const LLMS_TXT: &str = include_str!("../www/llms.txt");
const SECTION: &str = "## Tell the host what you needed";

fn all() -> Vec<String> {
    FeedbackKind::ALL.iter().map(|k| k.as_str().to_string()).collect()
}

fn schema_kinds() -> Vec<String> {
    let tool = host_tool_descriptors(&KindRegistry::default())
        .into_iter()
        .find(|t| t.name.as_ref() == "host.feedback.send")
        .expect("host.feedback.send is a host tool");
    tool.input_schema["properties"]["kind"]["enum"]
        .as_array()
        .expect("kind enum")
        .iter()
        .map(|v| v.as_str().expect("string").to_string())
        .collect()
}

/// The kinds named on the section's `Kinds:` sentence, in order.
fn llms_kinds() -> Vec<String> {
    let start = LLMS_TXT.find(SECTION).expect("llms.txt has the feedback section");
    let body = &LLMS_TXT[start + SECTION.len()..];
    let end = body.find("\n## ").unwrap_or(body.len());
    let body = &body[..end];
    let after = &body[body.find("Kinds: ").expect("a `Kinds:` line") + "Kinds: ".len()..];
    let line = after.split(". ").next().expect("sentence");
    line.split(", ").map(|k| k.trim_matches('`').to_string()).collect()
}

fn agree(a: &[String], b: &[String], c: &[String]) -> bool {
    a == b && b == c
}

#[test]
fn fbtl_kinds_cannot_drift() {
    let (all, schema, doc) = (all(), schema_kinds(), llms_kinds());
    assert!(agree(&all, &schema, &doc), "ALL {all:?} / schema {schema:?} / llms.txt {doc:?} must be identical");
    // The doc line is the rendered one, verbatim.
    assert!(LLMS_TXT.contains(&format!("Kinds: {}", FeedbackKind::llms_txt_kinds())));

    // Removing one kind from any of the three breaks agreement.
    let drop = |v: &[String]| v[1..].to_vec();
    assert!(!agree(&drop(&all), &schema, &doc), "ALL missing a kind must fail");
    assert!(!agree(&all, &drop(&schema), &doc), "schema missing a kind must fail");
    assert!(!agree(&all, &schema, &drop(&doc)), "llms.txt missing a kind must fail");
}
