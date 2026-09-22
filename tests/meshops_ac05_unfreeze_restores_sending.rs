//! PRD-mcphost-agent-mesh-ops
//! AC5 (P0) -- Given A is frozen, When the admin calls
//! `admin.mesh.unfreeze(A)`, Then A's next send succeeds and
//! `admin_events` holds one freeze and one unfreeze row for A.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use rusqlite::params;
use serde_json::json;

fn count_events(db_path: &std::path::Path, action: &str, tenant: &str) -> i64 {
    let conn = rusqlite::Connection::open(db_path).expect("open raw db");
    conn.query_row(
        "SELECT COUNT(*) FROM admin_events WHERE action = ?1 AND tenant = ?2",
        params![action, tenant],
        |r| r.get(0),
    )
    .expect("count admin_events")
}

#[tokio::test]
async fn ac5_unfreeze_restores_sending_and_audits_both_events() {
    let server = TestServer::start().await;
    let db_path = server.data_dir.0.join("mcphost.db");
    let (ns_a, key_a) = signup(&server.base_url, "Tenant A").await;
    let (ns_b, _key_b) = signup(&server.base_url, "Tenant B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    admin
        .tools_call("admin.mesh.freeze", json!({"tenant": ns_a.clone(), "reason": "flood"}))
        .await
        .expect("admin.mesh.freeze");

    let frozen_err = client_a
        .tools_call("host.msg.send", json!({"to": [ns_b.clone()], "body": "while frozen"}))
        .await
        .expect_err("send while frozen must fail");
    assert_eq!(frozen_err.error_code.as_deref(), Some("mesh_frozen"), "{frozen_err:?}");

    let unfreeze_raw = admin
        .tools_call("admin.mesh.unfreeze", json!({"tenant": ns_a.clone()}))
        .await
        .expect("admin.mesh.unfreeze");
    assert_eq!(extract_structured(&unfreeze_raw)["mesh_frozen"], json!(false));

    let send_raw = client_a
        .tools_call("host.msg.send", json!({"to": [ns_b], "body": "after unfreeze"}))
        .await
        .expect("send after unfreeze must succeed");
    assert!(extract_structured(&send_raw)["message_id"].is_string(), "{send_raw:?}");

    assert_eq!(count_events(&db_path, "mesh.freeze", &ns_a), 1);
    assert_eq!(count_events(&db_path, "mesh.unfreeze", &ns_a), 1);
}
