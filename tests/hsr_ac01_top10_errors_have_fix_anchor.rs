//! PRD-mcphost-help-search-tool
//! AC1 (P0) — Given `tests/fixtures/hsr_top10_errors.json` (the 10 most
//! frequent 10-08 explore error strings, each with its fix anchor), When each
//! string is sent as `q` with `k=3`, Then >= 8 of 10 responses contain the fix
//! anchor; a lower count fails the test with the misses listed.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::{Value, json};

pub(crate) fn fixture() -> Vec<(String, String)> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/hsr_top10_errors.json");
    let rows: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(path).expect("fixture readable"))
        .expect("fixture is a JSON array");
    rows.iter()
        .map(|r| (r["error"].as_str().expect("error").to_string(), r["anchor"].as_str().expect("anchor").to_string()))
        .collect()
}

#[tokio::test]
async fn at_least_eight_of_ten_errors_return_their_fix_anchor_in_top3() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    let rows = fixture();
    assert_eq!(rows.len(), 10, "the fixture is the ten most frequent errors");

    let mut misses = Vec::new();
    for (error, anchor) in &rows {
        let result = client
            .tools_call("host.help.search", json!({"q": error, "k": 3}))
            .await
            .unwrap_or_else(|e| panic!("host.help.search '{error}' failed: {} {}", e.code, e.message));
        let hits = extract_structured(&result)["hits"].as_array().cloned().unwrap_or_default();
        let anchors: Vec<&str> = hits.iter().filter_map(|h| h["anchor"].as_str()).collect();
        if !anchors.contains(&anchor.as_str()) {
            misses.push(format!("{error:?}: want {anchor}, got {anchors:?}"));
        }
    }
    assert!(
        rows.len() - misses.len() >= 8,
        "only {}/10 error strings returned their fix anchor in the top 3; misses:\n{}",
        rows.len() - misses.len(),
        misses.join("\n")
    );
}
