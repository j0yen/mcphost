//! contract: reads contracts/host-output-variants.v1.json
//! contract: reads registry handler::host_tool_descriptors
//! PRD-mcphost-output-schema-reach-sweep-live
//! AC5 (P1) -- Given a synthetic row with a `null` for an `integer` field and a
//! row missing a `required` key, When each is fed through the sweep's validator
//! path, Then each report names the tool, the instance path and the failing
//! keyword, the sweep's per-tool lines print calls, successes, variants seen and
//! unreached states, and `host.doctor` reports the unreached count and
//! `unreached_cap` from the variants file.

use std::collections::BTreeSet;

use crate::common::{McpClient, TestServer, extract_structured, signup};
use crate::osreach::{self, ToolRow};
use serde_json::{Value, json};

fn schema() -> Value {
    json!({"type": "object", "required": ["count", "label"], "properties": {"count": {"type": "integer"}, "label": {"type": "string"}}})
}

#[test]
fn osreach_ac05_a_null_for_an_integer_names_tool_path_and_keyword() {
    let msg = osreach::check("ctl.tool", "fresh", &schema(), &json!({"count": null, "label": "x"})).expect_err("rejected");
    for part in ["ctl.tool", "instance_path=/count", "keyword=type"] {
        assert!(msg.contains(part), "{part} missing from {msg}");
    }
}

#[test]
fn osreach_ac05_a_missing_required_key_names_tool_path_and_keyword() {
    let msg = osreach::check("ctl.tool", "dry_run", &schema(), &json!({"count": 1})).expect_err("rejected");
    for part in ["ctl.tool", "instance_path=/", "keyword=required"] {
        assert!(msg.contains(part), "{part} missing from {msg}");
    }
}

#[test]
fn osreach_ac05_the_per_tool_line_prints_calls_successes_variants_and_unreached() {
    let mut row = ToolRow { calls: 3, successes: 2, ..ToolRow::default() };
    row.variants.insert("dry_run".to_string());
    row.unreached.insert("fresh".to_string(), "not_found".to_string());
    let line = osreach::line_for("ctl.tool", &row);
    for part in ["ctl.tool", "calls=3", "successes=2", "variants=[dry_run]", "unreached=[fresh:not_found]"] {
        assert!(line.contains(part), "{part} missing from {line}");
    }
}

#[tokio::test]
async fn osreach_ac05_every_swept_tool_prints_a_line() {
    let sweep = osreach::sweep().await;
    let lines = sweep.lines();
    assert_eq!(lines.len(), sweep.rows.len());
    assert!(lines.iter().all(|l| l.contains("calls=") && l.contains("successes=") && l.contains("variants=[") && l.contains("unreached=[")));
}

#[tokio::test]
async fn osreach_ac05_host_doctor_reports_the_unreached_count_and_cap_from_the_variants_file() {
    let record: Value =
        serde_json::from_str(&std::fs::read_to_string(osreach::variants_file()).expect("variants file")).expect("json");
    let unreached: usize =
        record["tools"].as_object().expect("tools").values().map(|t| t["unreached"].as_array().map_or(0, Vec::len)).sum();
    let cap = record["unreached_cap"].as_u64().expect("unreached_cap");

    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Osreach AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let doctor = extract_structured(&client.tools_call("host.doctor", json!({})).await.expect("host.doctor"));
    assert_eq!(doctor["output_reach"]["unreached"], json!(unreached), "{doctor}");
    assert_eq!(doctor["output_reach"]["unreached_cap"], json!(cap), "{doctor}");
    let keys: BTreeSet<&str> = doctor.as_object().expect("object").keys().map(String::as_str).collect();
    assert!(keys.contains("error_envelope"), "existing fields are unchanged: {keys:?}");
}
