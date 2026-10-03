//! PRD-mcphost-trigger-set-idempotent
//! AC6 (P0) — Given a database with two same-kind triggers for one tool
//! before migration, When the migration runs, Then both survive with names
//! `<kind>:<tool>` and `<kind>:<tool>-2` and `list` shows both.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn migration_0064_backfills_names_for_pre_existing_duplicate_triggers() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC6 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .expect("find tenant")
        .expect("tenant exists");

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "pinger", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");

    // Simulate two pre-migration-0064 rows: same tenant, same kind, same
    // tool, `name` left NULL -- the exact duplicate the 2026-09-30 grounding
    // incident minted.
    let first_id = server
        .state
        .db
        .insert_legacy_trigger_without_name_for_test(tenant.id, "pinger".to_string(), "schedule".to_string())
        .await
        .expect("insert first legacy trigger");
    let second_id = server
        .state
        .db
        .insert_legacy_trigger_without_name_for_test(tenant.id, "pinger".to_string(), "schedule".to_string())
        .await
        .expect("insert second legacy trigger");

    server.state.db.migrate().await.expect("re-run migrations (0064 backfill)");

    let listed = extract_structured(
        &client
            .tools_call("host.trigger.list", json!({}))
            .await
            .expect("trigger.list"),
    );
    let triggers = listed["triggers"].as_array().expect("triggers array");
    assert_eq!(triggers.len(), 2, "both pre-existing duplicates must survive: {listed:?}");

    let names: std::collections::BTreeSet<&str> =
        triggers.iter().map(|t| t["name"].as_str().expect("name")).collect();
    assert_eq!(
        names,
        std::collections::BTreeSet::from(["schedule:pinger", "schedule:pinger-2"]),
        "{listed:?}"
    );

    let ids: std::collections::BTreeSet<&str> =
        triggers.iter().map(|t| t["id"].as_str().expect("id")).collect();
    assert_eq!(
        ids,
        std::collections::BTreeSet::from([first_id.as_str(), second_id.as_str()]),
        "ids must be unchanged by the backfill: {listed:?}"
    );

    // Idempotent: a second migrate() call leaves the same two names in
    // place (no further suffixing, no duplicate work).
    server.state.db.migrate().await.expect("re-run migrations again");
    let listed_again = extract_structured(
        &client
            .tools_call("host.trigger.list", json!({}))
            .await
            .expect("trigger.list"),
    );
    let triggers_again = listed_again["triggers"].as_array().expect("triggers array");
    assert_eq!(triggers_again.len(), 2, "{listed_again:?}");
    let names_again: std::collections::BTreeSet<&str> =
        triggers_again.iter().map(|t| t["name"].as_str().expect("name")).collect();
    assert_eq!(names_again, names, "re-running migrate() must not change already-backfilled names");
}
