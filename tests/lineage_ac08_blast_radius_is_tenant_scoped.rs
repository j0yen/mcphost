//! PRD-mcphost-lineage-blast-radius AC8 (P0) -- Given two tenants with a table of the same name, When
//! tenant A reads blast radius, Then no node from tenant B appears.

use crate::common;
use common::{TestServer, extract_structured, signup};
use mcphost::lineage::{self, NodeKind};
use serde_json::json;

#[tokio::test]
async fn blast_radius_never_leaks_another_tenants_consumer() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "Lineage AC8 Tenant A").await;
    let (ns_b, _key_b) = signup(&server.base_url, "Lineage AC8 Tenant B").await;
    let client_a = common::McpClient::with_bearer(&server.base_url, &key_a);

    let tenant_a = server
        .state
        .db
        .find_tenant_by_namespace(ns_a)
        .await
        .expect("db")
        .expect("tenant a exists");
    let tenant_b = server
        .state
        .db
        .find_tenant_by_namespace(ns_b)
        .await
        .expect("db")
        .expect("tenant b exists");

    lineage::register_edge(
        &server.state,
        tenant_a.id,
        (NodeKind::Table, "orders", "orders"),
        (NodeKind::Tool, "a_reader", "a_reader"),
        "declared_reads",
    )
    .await
    .expect("register tenant a edge");
    lineage::register_edge(
        &server.state,
        tenant_b.id,
        (NodeKind::Table, "orders", "orders"),
        (NodeKind::Tool, "b_reader", "b_reader"),
        "declared_reads",
    )
    .await
    .expect("register tenant b edge");

    let report = extract_structured(
        &client_a
            .tools_call("host.lineage.blast_radius", json!({"id": "table:orders", "change_kind": "drop"}))
            .await
            .expect("blast_radius"),
    );
    let impacted = report["impacted"].as_array().expect("impacted array");
    let ids: Vec<&str> = impacted.iter().map(|n| n["id"].as_str().unwrap()).collect();

    assert!(ids.contains(&"tool:a_reader"), "tenant A must see its own consumer: {ids:?}");
    assert!(!ids.contains(&"tool:b_reader"), "tenant A must never see tenant B's consumer: {ids:?}");

    let traced = extract_structured(
        &client_a
            .tools_call("host.lineage.trace", json!({"id": "table:orders"}))
            .await
            .expect("trace"),
    );
    let downstream = traced["downstream"].as_array().expect("downstream array");
    let downstream_ids: Vec<&str> = downstream.iter().map(|n| n["id"].as_str().unwrap()).collect();
    assert!(!downstream_ids.contains(&"tool:b_reader"), "trace must never show tenant B's node either: {downstream_ids:?}");
}
