//! PRD-mcphost-host-doctor-tool-b
//! AC2 -- Given 200 failed rows and `limit: 50`, When grouped, Then only the
//! newest 50 count, `window.failed` is 50, and groups are ordered by count
//! then newest `last_ts`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn only_the_newest_fifty_of_two_hundred_count_and_groups_order_by_count_then_recency() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "hostdocb ac2").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("lookup").expect("tenant");

    // Oldest 150 rows: one big `args_invalid` pile that must fall outside the
    // window. Newest 50, oldest first: 20 budget, 20 upstream, 10 timeout.
    // Budget and upstream tie on count; upstream has the newer `last_ts` and
    // must lead.
    let mut all: Vec<(&str, &str)> = Vec::new();
    all.extend(std::iter::repeat_n(("old", "args_invalid"), 150));
    all.extend(std::iter::repeat_n(("summarize", "budget_ceiling_exceeded"), 20));
    all.extend(std::iter::repeat_n(("fetch", "upstream_status"), 20));
    all.extend(std::iter::repeat_n(("slow", "call_timeout"), 10));
    assert_eq!(all.len(), 200);

    for (tool, class) in &all {
        server
            .state
            .db
            .record_call_traced(
                tenant.id,
                tool.to_string(),
                5,
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
                Some(format!("{class}: {class} happened")),
            )
            .await
            .expect("record failed call");
    }

    let client = McpClient::with_bearer(&server.base_url, &key);
    let out = extract_structured(&client.tools_call("host.doctor", json!({"limit": 50})).await.expect("doctor"));
    assert_eq!(out["window"]["failed"], json!(50), "{out}");
    let groups = out["groups"].as_array().expect("groups");
    let codes: Vec<&str> = groups.iter().map(|g| g["code"].as_str().unwrap()).collect();
    assert_eq!(codes, vec!["upstream_status", "budget_ceiling_exceeded", "call_timeout"], "{out}");
    let counts: Vec<i64> = groups.iter().map(|g| g["count"].as_i64().unwrap()).collect();
    assert_eq!(counts, vec![20, 20, 10], "{out}");
    assert!(groups.iter().all(|g| g["code"] != json!("args_invalid")), "old rows leaked in: {out}");
    assert!(groups[0]["last_ts"].as_str() >= groups[1]["last_ts"].as_str(), "{out}");

    // The default window (50) is the same; a 200 window sees the big pile first.
    let wide = extract_structured(&client.tools_call("host.doctor", json!({"limit": 200})).await.expect("doctor"));
    assert_eq!(wide["window"]["failed"], json!(200), "{wide}");
    assert_eq!(wide["groups"][0]["code"], json!("args_invalid"), "{wide}");
    assert_eq!(wide["groups"][0]["count"], json!(150), "{wide}");
}
