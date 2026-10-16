//! PRD-mcphost-host-doctor-next-names-envelope-key
//! AC4 -- Given a doctor window holding two envelope-index failures of one
//! tool, When `host.doctor` runs, Then one `tool_exception` group with
//! `count: 2`, a `first_source_line`, and a `next` naming
//! `mcphost.state.query` and `["rows"]` within `MAX_NEXT_CHARS`; and every
//! `DOCTOR_FIXTURES` row (including the envelope case) renders.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::doctor::{DOCTOR_FIXTURES, MAX_NEXT_CHARS, error_summary, family_of, parse_summary, render_next};
use serde_json::json;

#[tokio::test]
async fn hdnext_ac04() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "hdnext ac4").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("lookup").expect("tenant");

    let data = json!({
        "line": 3,
        "source_line": "return {\"first\": env[0][\"name\"]}",
        "envelope_fn": "mcphost.state.query",
        "envelope_keys": ["rows"],
    });
    let message = "KeyError: 0 -- mcphost.state.query returns a dict with keys [\"rows\"] at tool.py:3";
    let summary = error_summary("tool_exception", message, &data);
    for _ in 0..2 {
        server
            .state
            .db
            .record_call_traced(
                tenant.id,
                "envidx".to_string(),
                5,
                false,
                Some("tool_exception".to_string()),
                None,
                None,
                "error",
                tenant.origin.clone(),
                tenant.origin_detail.clone(),
                None,
                None,
                None,
                None,
                None,
                "key".to_string(),
                None,
                None,
                None,
                Some(summary.clone()),
            )
            .await
            .expect("record failed call");
    }

    let client = McpClient::with_bearer(&server.base_url, &key);
    let out = extract_structured(&client.tools_call("host.doctor", json!({})).await.expect("doctor"));
    let groups = out["groups"].as_array().expect("groups");
    assert_eq!(groups.len(), 1, "{out}");
    let g = &groups[0];
    assert_eq!(g["code"], json!("tool_exception"), "{out}");
    assert_eq!(g["family"], json!("tool_exception"), "{out}");
    assert_eq!(g["count"], json!(2), "{out}");
    assert_eq!(g["first_source_line"], json!("return {\"first\": env[0][\"name\"]}"), "{out}");
    let next = g["next"].as_str().expect("next");
    assert!(next.contains("mcphost.state.query") && next.contains("[\"rows\"]"), "{next}");
    assert!(next.chars().count() <= MAX_NEXT_CHARS, "{} chars: {next}", next.chars().count());

    // The fixture table carries the envelope case and each row renders clean.
    let (_, tool, fixture) = DOCTOR_FIXTURES
        .iter()
        .find(|(_, _, s)| s.contains("\nenvelope: "))
        .expect("DOCTOR_FIXTURES has an envelope row");
    let parsed = parse_summary(Some(fixture), "unknown");
    let rendered = render_next(family_of(&parsed), tool, &parsed);
    assert!(rendered.contains("mcphost.state.query") && rendered.contains("[\"rows\"]"), "{rendered}");
    assert!(!rendered.contains('{'), "{rendered}");
}
