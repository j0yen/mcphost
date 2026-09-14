//! PRD-mcphost-agent-directory
//! AC7 (P0) — Given A holds `@indexer` and is deleted through
//! `admin.tenant_delete`, When B claims `@indexer`, Then the claim
//! succeeds (proving `agent_profiles` carries no lingering row for A's
//! former id -- migration 0020's `ON DELETE CASCADE` on `tenant_id`, and
//! the unique index on `handle`, would otherwise refuse B's claim).
//! AC9 (P1) — Given a handle in the reserved list, When any tenant claims
//! it, Then the response is `handle_reserved`; and given the admin key
//! calls `admin.agent.handle_release("@indexer")`, Then the handle is free
//! and one `admin_events` row records the release.
//! AC10 (P1) — Given a tenant created with the `x-mcphost-synthetic`
//! header, When another tenant looks it up, Then its card carries
//! `synthetic: true`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use rusqlite::params;
use serde_json::json;

#[tokio::test]
async fn ac7_deleting_the_holder_frees_its_handle_immediately() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let (ns_a, key_a) = signup(&server.base_url, "Departing Agent").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    client_a
        .tools_call("host.agent.profile_set", json!({"handle": "indexer"}))
        .await
        .expect("A claims @indexer");

    admin
        .tools_call("admin.tenant_delete", json!({"tenant": ns_a}))
        .await
        .expect("admin.tenant_delete");

    let (_ns_b, key_b) = signup(&server.base_url, "Successor Agent").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    let raw = client_b
        .tools_call("host.agent.profile_set", json!({"handle": "indexer"}))
        .await
        .expect("B's claim of the now-freed handle must succeed");
    let profile = extract_structured(&raw);
    assert_eq!(profile["handle"], json!("indexer"), "{profile:?}");
}

#[tokio::test]
async fn ac9_reserved_handles_are_refused_and_admin_can_release() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let (_ns, key) = signup(&server.base_url, "Reservist").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let err = client
        .tools_call("host.agent.profile_set", json!({"handle": "admin"}))
        .await
        .expect_err("a reserved handle must be refused");
    assert_eq!(err.error_code.as_deref(), Some("handle_reserved"));

    // Now the release path, on a non-reserved handle.
    let (_ns_a, key_a) = signup(&server.base_url, "Holder").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    client_a
        .tools_call("host.agent.profile_set", json!({"handle": "indexer"}))
        .await
        .expect("A claims @indexer");

    let release_raw = admin
        .tools_call("admin.agent.handle_release", json!({"address": "@indexer"}))
        .await
        .expect("admin.agent.handle_release");
    let release = extract_structured(&release_raw);
    assert_eq!(release["released"], json!(true), "{release:?}");

    let db_path = server.data_dir.0.join("mcphost.db");
    let conn = rusqlite::Connection::open(&db_path).expect("open raw db");
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM admin_events WHERE action = ?1 AND detail = ?2",
            params!["agent_handle_release", "indexer"],
            |r| r.get(0),
        )
        .expect("count admin_events rows");
    assert_eq!(count, 1, "exactly one admin_events row must record the release");

    // The handle is free again immediately.
    let (_ns_b, key_b) = signup(&server.base_url, "New Claimant").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    let claim_raw = client_b
        .tools_call("host.agent.profile_set", json!({"handle": "indexer"}))
        .await
        .expect("B can reclaim the released handle");
    let profile = extract_structured(&claim_raw);
    assert_eq!(profile["handle"], json!("indexer"), "{profile:?}");
}

#[tokio::test]
async fn ac10_synthetic_tenants_card_carries_the_flag() {
    let server = TestServer::start().await;

    let signup_raw = McpClient::new(&server.base_url)
        .tools_call_with_header(
            "signup",
            json!({"name": "Harness Agent"}),
            ("x-mcphost-synthetic", "operator"),
        )
        .await
        .expect("synthetic signup");
    let signup_result = extract_structured(&signup_raw);
    let ns_synthetic = signup_result["tenant"].as_str().expect("tenant namespace").to_string();

    let (_ns_b, key_b) = signup(&server.base_url, "Real Looker").await;
    let looker = McpClient::with_bearer(&server.base_url, &key_b);
    let raw = looker
        .tools_call("host.agent.lookup", json!({"address": ns_synthetic}))
        .await
        .expect("lookup synthetic tenant");
    let card = extract_structured(&raw);
    assert_eq!(card["synthetic"], json!(true), "{card:?}");
}
