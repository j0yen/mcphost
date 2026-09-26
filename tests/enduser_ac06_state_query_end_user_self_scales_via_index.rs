//! PRD-mcphost-end-user-identity
//! AC6 (P0) — Given a table with 1000 rows (990 tenant-wide, 10 belonging
//! to end user u1), When u1 runs `host.state.query{end_user:"self"}`,
//! Then exactly 10 rows return, and `EXPLAIN QUERY PLAN` for the same
//! lookup shows the composite `(tenant_id, table_name, end_user_subject)`
//! index is used, not a full table scan.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use crate::oauth;
use oauth::{KID_1, jwk_1, jwks_server, priv_pem_1, sign};
use serde_json::json;

#[tokio::test]
async fn exactly_u1s_ten_rows_return_and_the_composite_index_is_used() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC6 Tenant").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

    let jwks = jwks_server(jwk_1()).await;
    let issuer = "https://issuer.example.com";
    let audience = "mcphost-test-audience";
    key_client
        .tools_call(
            "host.oauth.issuer_set",
            json!({"issuer": issuer, "audience": audience, "jwks_url": format!("{}/jwks", jwks.uri())}),
        )
        .await
        .expect("issuer_set must succeed");
    let token_u1 = sign(KID_1, priv_pem_1(), issuer, audience, "u1", 300);
    let client_u1 = McpClient::with_bearer(&server.base_url, &token_u1);

    key_client
        .tools_call(
            "host.state.table_create",
            json!({
                "name": "bigtable",
                "schema": {"id": "text", "v": "integer"},
                "primary_key": "id",
            }),
        )
        .await
        .expect("table_create");

    // 990 tenant-wide rows, batched under the free plan's
    // state_ops_per_call_max (200 rows/call).
    let mut inserted = 0usize;
    while inserted < 990 {
        let batch_size = 198.min(990 - inserted);
        let rows: Vec<serde_json::Value> = (0..batch_size)
            .map(|i| json!({"id": format!("tenant-{}", inserted + i), "v": (inserted + i) as i64}))
            .collect();
        key_client
            .tools_call("host.state.insert", json!({"table": "bigtable", "rows": rows}))
            .await
            .expect("tenant-wide insert batch");
        inserted += batch_size;
    }

    // 10 rows belonging to u1.
    let u1_rows: Vec<serde_json::Value> =
        (0..10).map(|i| json!({"id": format!("u1-{i}"), "v": 1000 + i})).collect();
    client_u1
        .tools_call(
            "host.state.insert",
            json!({"table": "bigtable", "rows": u1_rows, "end_user": "self"}),
        )
        .await
        .expect("u1 insert");

    let result = extract_structured(
        &client_u1
            .tools_call("host.state.query", json!({"table": "bigtable", "end_user": "self"}))
            .await
            .expect("u1 query"),
    );
    let rows = result["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 10, "expected exactly u1's own 10 rows: {rows:?}");
    for row in rows {
        let id = row["id"].as_str().unwrap_or("");
        assert!(id.starts_with("u1-"), "a non-u1 row leaked into u1's scoped query: {row:?}");
    }

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .unwrap()
        .expect("tenant");
    let plan_steps = server
        .state
        .db
        .explain_state_rows_for_end_user_for_test(tenant.id, "bigtable".to_string(), "u1".to_string())
        .await
        .expect("explain query plan");
    let plan_text = plan_steps.join(" | ");
    assert!(
        plan_text.contains("idx_tenant_state_rows_tenant_table_enduser"),
        "expected the composite end-user index in the query plan, got: {plan_text}"
    );
    let plan_upper = plan_text.to_uppercase();
    assert!(plan_upper.contains("SEARCH"), "expected an index SEARCH step: {plan_text}");
    assert!(!plan_upper.contains("SCAN"), "expected no full table SCAN step: {plan_text}");
}
