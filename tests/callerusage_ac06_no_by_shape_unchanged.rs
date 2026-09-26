//! PRD-mcphost-shared-tool-caller-usage
//! AC6 (P0) — Given `host.usage` without `by`, When called, Then the
//! output matches the pre-PRD per-tenant shape byte for byte on the
//! existing fixture.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;
use std::collections::BTreeSet;

#[tokio::test]
async fn usage_without_by_keeps_the_pre_prd_shape() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC6 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echoer", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish echoer");

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("db query")
        .expect("tenant exists");
    server
        .state
        .db
        .record_call(tenant.id, "echoer".to_string(), 5, true, None, None, None, "ok", "external".to_string(), None)
        .await
        .expect("seed ok call");
    server
        .state
        .db
        .record_call(
            tenant.id,
            "echoer".to_string(),
            5,
            false,
            Some("kind_error".to_string()),
            None,
            None,
            "error",
            "external".to_string(),
            None,
        )
        .await
        .expect("seed error call");

    let usage = client
        .tools_call("host.usage", json!({}))
        .await
        .expect("host.usage with no by");
    let body = extract_structured(&usage);
    let obj = body.as_object().expect("host.usage returns an object");

    // The exact pre-PRD field set -- this PRD's own migration/compatibility
    // note: "host.usage's existing per-tenant output is unchanged when
    // `by` is omitted." Neither `by`, `rows`, nor `cursor` (the new
    // breakdown fields) may appear here. `run_results_bytes` is included
    // because PRD-mcphost-run-result-overflow-to-state (landed on main
    // ahead of this branch's own rebase) added it to this same per-tenant
    // shape -- it predates this PRD's own AC6 fixture but not this test's
    // original authorship, so it belongs in the unchanged set alongside
    // every other pre-existing field.
    let expected_keys: BTreeSet<&str> = [
        "window",
        "calls",
        "errors",
        "p50_ms",
        "p95_ms",
        "state_bytes",
        "capacity_refusals",
        "calls_by_others",
        "calls_to_shared",
        "jobs",
        "scheduled",
        "retention_days",
        "run_results_bytes",
    ]
    .into_iter()
    .collect();
    let actual_keys: BTreeSet<&str> = obj.keys().map(String::as_str).collect();
    assert_eq!(
        actual_keys, expected_keys,
        "host.usage's no-by shape must not gain or lose a top-level field"
    );

    assert_eq!(body["calls"], json!(2));
    assert_eq!(body["errors"], json!(1));
    let jobs = body["jobs"].as_object().expect("jobs object");
    let jobs_keys: BTreeSet<&str> = jobs.keys().map(String::as_str).collect();
    assert_eq!(
        jobs_keys,
        ["done", "error", "timeout", "cancelled", "seconds"].into_iter().collect(),
        "jobs sub-shape must be unchanged"
    );
    let scheduled = body["scheduled"].as_object().expect("scheduled object");
    let scheduled_keys: BTreeSet<&str> = scheduled.keys().map(String::as_str).collect();
    assert_eq!(
        scheduled_keys,
        ["done", "error", "timeout", "cancelled", "skipped", "seconds"]
            .into_iter()
            .collect(),
        "scheduled sub-shape must be unchanged"
    );
}
