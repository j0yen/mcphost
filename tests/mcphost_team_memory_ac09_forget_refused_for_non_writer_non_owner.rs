//! PRD-mcphost-team-memory
//! AC9 — Given `forget {id}` by a tenant that is neither writer nor
//! owner, When called, Then it is refused.
//!
//! `forget` (P2 requirement 7) is not part of the P0 recipe
//! (`www/llms.txt`'s "Give your agents one memory" section names exactly
//! the table, two tools, group creation, two shares, and the add call --
//! AC1), so this test publishes and shares it directly rather than going
//! through `examples/team-memory/proof.sh`. `mcphost.table` has no
//! row-level delete (only create/append/query/list/drop/schema --
//! `src/tables.rs`), so `examples/team-memory/tools/forget.py` records a
//! tombstone in a companion `memory_forgotten` table instead of deleting;
//! this test only proves the permission check requirement 7/AC9 names.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn forget_is_refused_for_a_tenant_that_is_neither_writer_nor_owner() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;

    let (owner_ns, owner_key) = signup(&server.base_url, "AC9 Owner").await;
    let (b_ns, b_key) = signup(&server.base_url, "AC9 Writer").await;
    let (d_ns, d_key) = signup(&server.base_url, "AC9 Neither").await;
    let owner = McpClient::with_bearer(&server.base_url, &owner_key);
    let b = McpClient::with_bearer(&server.base_url, &b_key);
    let d = McpClient::with_bearer(&server.base_url, &d_key);

    owner
        .tools_call(
            "host.table.create",
            json!({
                "name": "memory",
                "columns": {"key": "text", "text": "text", "tags": "json", "writer": "text", "at": "real"},
            }),
        )
        .await
        .expect("table create");

    let remember_source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/team-memory/tools/remember.py"),
    )
    .expect("read remember.py");
    owner
        .tools_call(
            "host.tool_publish",
            json!({"name": "remember", "kind": "python", "spec": {"source": remember_source}}),
        )
        .await
        .expect("publish remember");

    let forget_source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/team-memory/tools/forget.py"),
    )
    .expect("read forget.py")
    .replace("__OWNER_NAMESPACE__", &owner_ns);
    owner
        .tools_call(
            "host.tool_publish",
            json!({"name": "forget", "kind": "python", "spec": {"source": forget_source}}),
        )
        .await
        .expect("publish forget");

    owner
        .tools_call("host.group.create", json!({"name": "team"}))
        .await
        .expect("group create");
    owner
        .tools_call(
            "host.tool_share",
            json!({"name": "remember", "visibility": "group", "group": "team"}),
        )
        .await
        .expect("share remember");
    owner
        .tools_call(
            "host.tool_share",
            json!({"name": "forget", "visibility": "group", "group": "team"}),
        )
        .await
        .expect("share forget");
    owner
        .tools_call("host.group.add", json!({"name": "team", "namespace": b_ns}))
        .await
        .expect("group add b");
    owner
        .tools_call("host.group.add", json!({"name": "team", "namespace": d_ns}))
        .await
        .expect("group add d");

    let remembered = poll_until_ready(
        &b,
        &format!("{owner_ns}.remember"),
        json!({"key": "deploy", "text": "deploy window is Tuesday", "who": b_ns}),
        Duration::from_secs(10),
    )
    .await
    .unwrap_or_else(|e| panic!("b's remember must succeed: {} {}", e.code, e.message));
    let row_id = extract_structured(&remembered)["id"].clone();

    let forget_qualified = format!("{owner_ns}.forget");

    // D is neither the row's writer (B) nor the owner -- refused.
    let d_forget = poll_until_ready(&d, &forget_qualified, json!({"id": row_id, "who": d_ns}), Duration::from_secs(10)).await;
    assert!(
        d_forget.is_err(),
        "forget must be refused for a tenant that is neither writer nor owner, got: {d_forget:?}"
    );

    // Positive control: the writer's own forget of the same row succeeds
    // -- proves the check discriminates rather than always refusing.
    let b_forget = poll_until_ready(&b, &forget_qualified, json!({"id": row_id, "who": b_ns}), Duration::from_secs(10))
        .await
        .unwrap_or_else(|e| panic!("b's own forget must succeed: {} {}", e.code, e.message));
    assert_eq!(extract_structured(&b_forget)["ok"], json!(true));
}
