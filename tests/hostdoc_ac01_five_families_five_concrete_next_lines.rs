//! PRD-mcphost-host-doctor-tool
//! AC1 -- Given a fixture tenant with 5 failed calls per 10-08 family (budget
//! ceiling, string-indices `tool_exception`, NameError `tool_exception`,
//! `args_invalid` 4.78, `upstream_status` 403), When `host.doctor` runs, Then
//! it returns 5 groups and every `next` names a concrete call, argument, line,
//! or value, none equal to a bare template.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::doctor::DOCTOR_NEXT;
use serde_json::{Value, json};

/// `(tool, error_class, error_summary)` -- one per 10-08 explore family.
const FAMILIES: &[(&str, &str, &str)] = &[
    (
        "summarize",
        "budget_ceiling_exceeded",
        "budget_ceiling_exceeded: budget.max_child_calls: 99999 exceeds this plan's ceiling of 30000",
    ),
    (
        "lookup",
        "tool_exception",
        "tool_exception: TypeError: string indices must be integers, not 'str' at tool.py:16\nname = rows[\"name\"]",
    ),
    (
        "report",
        "tool_exception",
        "tool_exception: NameError: name 'mcphost' is not defined at tool.py:7\nrows = mcphost.state.query(table=T)",
    ),
    ("count_rows", "args_invalid", "args_invalid: args.limit: expected integer, got number"),
    ("fetch_issue", "upstream_status", "upstream_status: upstream returned HTTP 403 (upstream_status)"),
];

#[tokio::test]
async fn five_families_give_five_groups_each_with_a_concrete_next() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "hostdoc ac1").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("lookup").expect("tenant");
    for (tool, class, summary) in FAMILIES {
        for _ in 0..5 {
            server
                .state
                .db
                .record_call_traced(
                    tenant.id,
                    tool.to_string(),
                    12,
                    false,
                    Some(class.to_string()),
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
                    Some(summary.to_string()),
                )
                .await
                .expect("record failed call");
        }
    }

    let client = McpClient::with_bearer(&server.base_url, &key);
    let out: Value = extract_structured(&client.tools_call("host.doctor", json!({})).await.expect("host.doctor"));
    assert_eq!(out["window"]["failed"], json!(25), "{out}");
    let groups = out["groups"].as_array().expect("groups");
    assert_eq!(groups.len(), 5, "five families, five groups: {out}");

    let next_of = |tool: &str| -> String {
        let g = groups
            .iter()
            .find(|g| g["tools"].as_array().is_some_and(|t| t.contains(&json!(tool))))
            .unwrap_or_else(|| panic!("no group for {tool}: {out}"));
        assert_eq!(g["count"], json!(5), "{tool}: {g}");
        g["next"].as_str().expect("next").to_string()
    };
    // The fixture's own substitutions: {tool}, {ceiling}, {argument}, {line},
    // {expected_type}, {upstream_status}.
    let budget = next_of("summarize");
    assert!(budget.contains("`summarize`") && budget.contains("budget.max_child_calls") && budget.contains("30000"), "{budget}");
    let type_error = next_of("lookup");
    assert!(type_error.contains("`lookup`") && type_error.contains("tool.py:16"), "{type_error}");
    let name_error = next_of("report");
    assert!(name_error.contains("`report`") && name_error.contains("tool.py:7"), "{name_error}");
    let args = next_of("count_rows");
    assert!(args.contains("`count_rows`") && args.contains("limit") && args.contains("integer"), "{args}");
    let upstream = next_of("fetch_issue");
    assert!(upstream.contains("`fetch_issue`") && upstream.contains("HTTP 403"), "{upstream}");

    for g in groups {
        let next = g["next"].as_str().expect("next");
        assert!(!next.contains('{'), "unfilled placeholder: {next}");
        assert!(DOCTOR_NEXT.iter().all(|(_, template)| *template != next), "bare template: {next}");
    }
    // R4: both tool_exception groups carry the newest source line.
    let src = groups.iter().find(|g| g["class"] == json!("TypeError")).expect("TypeError group");
    assert_eq!(src["first_source_line"], json!("name = rows[\"name\"]"), "{src}");
    assert_eq!(src["line"], json!(16), "{src}");
}
