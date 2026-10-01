//! PRD-mcphost-lineage-blast-radius AC7 (P0) -- Given 150 downstream consumers, When trace is called, Then
//! a `handle` returns with `downstream_count` 150 and `trace_page` returns
//! them 50 at a time.

use crate::common;
use common::{TestServer, extract_structured, signup};
use mcphost::lineage::{self, NodeKind};
use serde_json::json;
use std::collections::BTreeSet;

#[tokio::test]
async fn trace_pages_150_downstream_consumers_50_at_a_time() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Lineage AC7 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("db")
        .expect("tenant exists");

    for i in 0..150 {
        let name = format!("consumer_{i}");
        lineage::register_edge(
            &server.state,
            tenant.id,
            (NodeKind::Table, "big", "big"),
            (NodeKind::Tool, &name, &name),
            "declared_reads",
        )
        .await
        .expect("register edge");
    }

    let traced = extract_structured(
        &client
            .tools_call("host.lineage.trace", json!({"id": "table:big"}))
            .await
            .expect("trace table:big"),
    );
    assert_eq!(traced["downstream_count"], json!(150), "traced: {traced:?}");
    assert!(traced.get("downstream").is_none(), "over the inline limit, downstream must not be inlined: {traced:?}");
    let handle = traced["downstream_handle"].as_str().expect("downstream_handle").to_string();

    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut offset = 0i64;
    let mut pages = 0;
    loop {
        let page = extract_structured(
            &client
                .tools_call("host.lineage.trace_page", json!({"handle": handle, "offset": offset}))
                .await
                .expect("trace_page"),
        );
        assert_eq!(page["total"], json!(150), "page at offset {offset}: {page:?}");
        let entries = page["entries"].as_array().expect("entries array");
        pages += 1;
        if pages <= 3 {
            assert_eq!(entries.len(), 50, "page {pages} (offset {offset}) must carry 50 entries: {page:?}");
        }
        for e in entries {
            seen.insert(e["id"].as_str().unwrap().to_string());
        }
        match page["next_offset"].as_i64() {
            Some(next) => offset = next,
            None => break,
        }
        assert!(pages <= 10, "trace_page did not terminate: {page:?}");
    }
    assert_eq!(pages, 3, "150 entries at 50/page must be exactly 3 pages");
    assert_eq!(seen.len(), 150, "every consumer must be seen exactly once across pages");
    for i in 0..150 {
        assert!(seen.contains(&format!("tool:consumer_{i}")), "missing consumer_{i}");
    }
}
