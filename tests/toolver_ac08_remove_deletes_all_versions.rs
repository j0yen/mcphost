//! AC8 — Given `host.tool_remove`, When called, Then all versions are
//! gone and `host.tool_history` returns not-found.
//!
//! `host.tool_history`'s not-found comes solely from the parent `tools`
//! row lookup (`Db::get_tool` queries only `tools`), so that assertion
//! alone would still pass even if `Db::remove_tool` stopped deleting
//! `tool_versions`/`shared_tool_last_seen` rows. This test also reaches
//! into the DB directly (as `signup_and_make_pro` and friends already do
//! elsewhere in this suite) to pin that those rows are actually gone, not
//! just orphaned behind a deleted `tools` row.

use crate::common;
use common::{McpClient, TestServer, publish, signup};
use serde_json::json;

#[tokio::test]
async fn remove_takes_every_stored_version_with_it() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Publisher").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let owner = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .expect("find owner tenant")
        .expect("owner tenant exists");

    publish(&client, "greet", "echo", json!({"schema": {"type": "object"}})).await;
    publish(&client, "greet", "echo", json!({"schema": {"type": "object"}})).await;

    // A shared, unpinned cross-tenant caller so remove also has a
    // `shared_tool_last_seen` row to take with it (requirement 7 names
    // both tables explicitly).
    client
        .tools_call("host.tool_share", json!({"name": "greet", "visibility": "public"}))
        .await
        .expect("share greet publicly");
    let (_ns_b, key_b) = signup(&server.base_url, "Caller").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    let caller = server
        .state
        .db
        .find_tenant_by_namespace(_ns_b.clone())
        .await
        .expect("find caller tenant")
        .expect("caller tenant exists");
    client_b
        .tools_call(&format!("{ns}.greet"), json!({}))
        .await
        .expect("B's unpinned call establishes a shared_tool_last_seen row");

    // Sanity on the fixture itself: both tables actually hold rows for
    // "greet" before remove runs, so the post-remove empty reads below
    // prove a deletion happened rather than an always-empty table.
    assert_eq!(
        server
            .state
            .db
            .list_tool_versions(owner.id, "greet".to_string())
            .await
            .expect("list_tool_versions before remove")
            .len(),
        2,
        "fixture must have two stored versions before remove"
    );
    assert!(
        server
            .state
            .db
            .shared_tool_last_seen(owner.id, "greet".to_string(), caller.id)
            .await
            .expect("shared_tool_last_seen before remove")
            .is_some(),
        "fixture must have a shared_tool_last_seen row before remove"
    );

    client
        .tools_call("host.tool_remove", json!({"name": "greet"}))
        .await
        .expect("remove greet");

    let err = client
        .tools_call("host.tool_history", json!({"name": "greet"}))
        .await
        .expect_err("history on a removed tool must be not-found");
    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"));

    // The part `host.tool_history`'s not-found alone can't prove: every
    // stored version, and the caller's version bookkeeping, are actually
    // gone from the DB -- not just unreachable behind a deleted `tools`
    // row.
    let remaining_versions = server
        .state
        .db
        .list_tool_versions(owner.id, "greet".to_string())
        .await
        .expect("list_tool_versions after remove");
    assert!(
        remaining_versions.is_empty(),
        "remove must delete every tool_versions row, found {remaining_versions:?}"
    );
    let remaining_last_seen = server
        .state
        .db
        .shared_tool_last_seen(owner.id, "greet".to_string(), caller.id)
        .await
        .expect("shared_tool_last_seen after remove");
    assert!(
        remaining_last_seen.is_none(),
        "remove must delete the shared_tool_last_seen row, found {remaining_last_seen:?}"
    );
}
