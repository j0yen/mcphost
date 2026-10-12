//! PRD-mcphost-destructive-verb-confirmation-token
//! AC6 -- Given a scratch tenant key, When `host.table.drop {name}` on a
//! 5 000-row table is called without `confirm`, Then it succeeds and the
//! response has `_meta.scratch == true` and no `confirm` field.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn seed_table(client: &McpClient, name: &str) {
    client
        .tools_call("host.table.create", json!({"name": name, "columns": {"sku": "text"}}))
        .await
        .expect("create");
    let rows: Vec<Value> = (0..5_000).map(|i| json!({"sku": format!("sku-{i}")})).collect();
    client
        .tools_call("host.table.append", json!({"table": name, "rows": rows}))
        .await
        .expect("append 5000 rows");
}

#[tokio::test]
async fn a_scratch_tenant_drops_a_big_table_without_a_token() {
    let server = TestServer::start().await;

    // A real tenant is gated: the same drop is refused until confirmed.
    let (_ns, real_key) = signup(&server.base_url, "Cnfm AC6 Real").await;
    let real = McpClient::with_bearer(&server.base_url, &real_key);
    seed_table(&real, "orders").await;
    let refused = real
        .tools_call("host.table.drop", json!({"name": "orders"}))
        .await
        .expect_err("a real tenant's 5 000-row drop needs a token");
    assert_eq!(refused.error_code.as_deref(), Some("confirm_required"), "{refused:?}");

    // A scratch tenant (`scratch_of` set) is never gated.
    let (ns, scratch_key) = signup(&server.base_url, "Cnfm AC6 Scratch").await;
    let scratch = McpClient::with_bearer(&server.base_url, &scratch_key);
    seed_table(&scratch, "orders").await;
    let conn = rusqlite::Connection::open(server.state.db.data_dir().join("mcphost.db")).expect("open db");
    let marked = conn
        .execute(
            "UPDATE tenants SET scratch_of = (SELECT id FROM tenants WHERE namespace != ?1 LIMIT 1) \
             WHERE namespace = ?1",
            [&ns],
        )
        .expect("mark the tenant as a scratch");
    assert_eq!(marked, 1);

    let dry = extract_structured(
        &scratch
            .tools_call("host.table.drop", json!({"name": "orders", "dry_run": true}))
            .await
            .expect("scratch dry run"),
    );
    assert_eq!(dry["_meta"]["scratch"], json!(true), "{dry}");
    assert!(dry.get("confirm").is_none(), "a scratch dry run mints no token: {dry}");

    let dropped = extract_structured(
        &scratch
            .tools_call("host.table.drop", json!({"name": "orders"}))
            .await
            .expect("a scratch drop runs without confirm"),
    );
    assert_eq!(dropped["dropped"], json!(true), "{dropped}");
    assert_eq!(dropped["_meta"]["scratch"], json!(true), "{dropped}");
    assert!(dropped.get("confirm").is_none(), "{dropped}");
}
