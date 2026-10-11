//! PRD-mcphost-uptime-probe-recipe-green
//! AC1 -- Given a free tenant and three fixture URLs (two answering 200, one
//! refusing), When `host.uptime.create(name="status", urls=[...])` is called
//! once, Then the response carries `status_tool: "status"`, `schedule_id`,
//! `every_s: 300`, `first_result.rows` of length 3 with `ok` true/true/false,
//! no tenant tool has a `network` field, and `host.trigger.list` shows one
//! schedule.

use crate::common;
use crate::uprg;

use common::extract_structured;
use serde_json::json;

#[tokio::test]
async fn one_create_call_yields_status_tool_schedule_and_first_rows() {
    let fx = uprg::start().await;
    let out = fx.create_status().await;

    assert_eq!(out["status_tool"], json!("status"), "{out}");
    assert!(out["schedule_id"].as_str().is_some_and(|s| !s.is_empty()), "{out}");
    assert_eq!(out["every_s"], json!(300), "{out}");
    let rows = out["first_result"]["rows"].as_array().unwrap_or_else(|| panic!("no rows: {out}"));
    assert_eq!(rows.len(), 3, "{out}");
    let oks: Vec<_> = rows.iter().map(|r| r["ok"].clone()).collect();
    assert_eq!(oks, vec![json!(true), json!(true), json!(false)], "{out}");

    // No tenant tool carries a `network` field.
    let tools = extract_structured(&fx.client.tools_call("host.tool_list", json!({})).await.expect("tool_list"));
    let tools = tools["tools"].as_array().unwrap_or_else(|| panic!("tool_list shape: {tools}"));
    assert!(tools.iter().any(|t| t["name"].as_str().is_some_and(|n| n.ends_with(".status"))), "{tools:?}");
    for tool in tools {
        let text = tool.to_string();
        assert!(!text.contains("\"network\""), "tool carries a network field: {text}");
    }

    // One schedule.
    let triggers = extract_structured(&fx.client.tools_call("host.trigger.list", json!({})).await.expect("trigger.list"));
    let schedules: Vec<_> = triggers["triggers"]
        .as_array()
        .expect("triggers")
        .iter()
        .filter(|t| t["kind"] == json!("schedule"))
        .collect();
    assert_eq!(schedules.len(), 1, "{triggers}");
    assert_eq!(schedules[0]["id"], out["schedule_id"], "{triggers}");
}
