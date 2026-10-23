//! PRD-mcphost-admin-funnel-real-surface-recipe-counter
//! AC4 (P0) — Given a migration, When `host.quickstart` or `host.tool_publish`
//! resolves an alias to a recipe, Then one `recipe_uses` row records tenant,
//! recipe, via and time, concurrent uses record one row each, a direct kind
//! none, and an injected insert failure leaves the response unchanged.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::{Value, json};

async fn tenant_id(server: &TestServer, key: &str) -> i64 {
    let whoami = extract_structured(&McpClient::with_bearer(&server.base_url, key).tools_call("host.whoami", json!({})).await.expect("whoami"));
    let ns = whoami["namespace"].as_str().expect("namespace").to_string();
    server.state.db.find_tenant_by_namespace(ns).await.expect("lookup").expect("tenant").id
}

#[tokio::test]
async fn alias_resolution_records_one_row_per_use_and_a_failed_insert_changes_nothing() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Recipe Counter AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tid = tenant_id(&server, &key).await;
    let before = mcphost::state::now_unix();

    // quickstart through an alias: one row, with tenant, recipe, via, time.
    let qs = extract_structured(&client.tools_call("host.quickstart", json!({"kind": "webhook"})).await.expect("quickstart"));
    let recipe = qs["recipe"]["name"].as_str().expect("recipe name").to_string();
    let rows = server.state.db.recipe_uses_for_test().await.expect("rows");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!((rows[0].0, rows[0].1.as_str(), rows[0].2.as_str()), (tid, recipe.as_str(), "quickstart"));
    assert!(rows[0].3 >= before && rows[0].3 <= mcphost::state::now_unix() + 1, "{rows:?}");

    // publish through an alias: one more row, via `publish`.
    let spec = json!({"method": "GET", "url": "http://127.0.0.1:9/ping"});
    let published = extract_structured(
        &client
            .tools_call("host.tool_publish", json!({"name": "on_event", "kind": "event", "spec": spec}))
            .await
            .expect("publish"),
    );
    assert_eq!(published["resolved_from"], json!("event"));
    let rows = server.state.db.recipe_uses_for_test().await.expect("rows");
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!((rows[1].0, rows[1].2.as_str()), (tid, "publish"));

    // A direct kind records nothing (quickstart and publish alike).
    client.tools_call("host.quickstart", json!({"kind": "http"})).await.expect("direct quickstart");
    client
        .tools_call("host.tool_publish", json!({"name": "direct", "kind": "http", "spec": spec}))
        .await
        .expect("direct publish");
    assert_eq!(server.state.db.recipe_uses_for_test().await.expect("rows").len(), 2);

    // Concurrent uses record one row each.
    let calls = (0..5).map(|_| client.tools_call("host.quickstart", json!({"kind": "webhook"})));
    for r in futures_util::future::join_all(calls).await {
        r.expect("concurrent quickstart");
    }
    assert_eq!(server.state.db.recipe_uses_for_test().await.expect("rows").len(), 7);

    // An injected insert failure (the table is gone) leaves the response
    // unchanged.
    let strip = |mut v: Value| {
        v.as_object_mut().map(|m| m.remove("server_time"));
        v
    };
    let ok = strip(extract_structured(&client.tools_call("host.quickstart", json!({"kind": "webhook"})).await.expect("ok")));
    server.state.db.drop_recipe_uses_for_test().await.expect("drop");
    let failed = strip(extract_structured(&client.tools_call("host.quickstart", json!({"kind": "webhook"})).await.expect("still ok")));
    assert_eq!(ok, failed);
    client
        .tools_call("host.tool_publish", json!({"name": "again", "kind": "event", "spec": spec}))
        .await
        .expect("publish still ok with the insert failing");
}
