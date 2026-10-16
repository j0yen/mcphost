//! PRD-mcphost-jq-kind
//! AC1 -- Given `tests/fixtures/jq_manual_examples.json` with 40
//! program/input/expected triples from the jq 1.7 manual, When each runs
//! under the kind, Then at least 38 match exactly and the two allowed misses
//! are named in the test with the manual section.

use mcphost::kinds::jq::JqKind;
use mcphost::kinds::{CallCtx, Kind};
use serde_json::{Value, json};

const FIXTURE: &str = include_str!("fixtures/jq_manual_examples.json");

/// The two allowed misses (manual section -> why `jaq` differs from `jq`).
const ALLOWED_MISSES: &[(&str, &str)] = &[
    (
        "Format strings and escaping: @base32",
        "jaq has no @base32 format (jq 1.7 manual, \"Format strings and escaping\")",
    ),
    (
        "Identity: . (large number literals keep their digits)",
        "jaq reads 100000000000000000000000 as the float 1e23; jq 1.7 preserves the literal (manual, \"Identity\")",
    ),
];

fn as_kind_result(expected: &[Value]) -> Value {
    match expected {
        [one] => one.clone(),
        many => Value::Array(many.to_vec()),
    }
}

#[tokio::test]
async fn jqk_ac01_manual_examples_match_under_the_kind() {
    let examples: Vec<Value> = serde_json::from_str(FIXTURE).expect("fixture is valid JSON");
    assert_eq!(examples.len(), 40, "the fixture holds exactly 40 manual examples");
    let kind = JqKind;
    let ctx = CallCtx::for_test(1, "t_jqk_ac01");
    let mut misses: Vec<(String, String)> = Vec::new();
    for ex in &examples {
        let (section, program) = (ex["section"].as_str().unwrap(), ex["program"].as_str().unwrap());
        let spec = json!({"program": program, "args_schema": {}});
        let want = as_kind_result(ex["expected"].as_array().unwrap());
        let got = match kind.validate(&spec) {
            Ok(()) => kind.call(&spec, ex["input"].clone(), &ctx).await.map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        if got.as_ref() != Ok(&want) {
            misses.push((section.to_string(), format!("`{program}`: got {got:?}, want {want}")));
        }
    }
    let unexpected: Vec<_> = misses
        .iter()
        .filter(|(section, _)| !ALLOWED_MISSES.iter().any(|(allowed, _)| allowed == section))
        .collect();
    assert!(unexpected.is_empty(), "misses outside the allowed set: {unexpected:#?}");
    assert!(
        examples.len() - misses.len() >= 38,
        "at least 38 of 40 must match; misses: {misses:#?}"
    );
    let missed: Vec<&str> = misses.iter().map(|(section, _)| section.as_str()).collect();
    for (section, why) in ALLOWED_MISSES {
        assert!(missed.contains(section), "allowed miss `{section}` ({why}) now matches: drop it");
    }
}
