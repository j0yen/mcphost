//! PRD-mcphost-agent-directory
//! AC1 (P0) — Given a fresh tenant with no profile row, When it calls
//! `host.agent.whoami`, Then the result has `address` equal to its
//! namespace, `handle: null`, `contact_policy: "open"`, and no `key_hash`,
//! `billing_ref` or `stripe_customer_id` key.
//! AC2 (P0) — Given tenant A claims `@indexer` via
//! `host.agent.profile_set`, When tenant B calls
//! `host.agent.lookup("@indexer")`, Then B receives A's card with
//! `address` equal to A's namespace and the card omits every billing and
//! key field.
//! AC3 (P0) — Given `@indexer` is held by A, When B calls
//! `host.agent.profile_set(handle="Indexer")`, Then B gets structured
//! error `handle_taken` whose body names no tenant, and A's handle is
//! unchanged.
//! AC4 (P0) — Given a namespace that does not exist, a tenant disabled via
//! `admin.tenant_disable`, and a tenant deleted via `admin.tenant_delete`,
//! When another tenant looks each up, Then all three responses are
//! `agent_not_found` with byte-identical bodies.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn ac1_whoami_defaults_for_a_profile_less_tenant() {
    let server = TestServer::start().await;
    let (namespace, key) = signup(&server.base_url, "Fresh Agent").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let raw = client
        .tools_call("host.agent.whoami", json!({}))
        .await
        .expect("host.agent.whoami");
    let whoami = extract_structured(&raw);

    assert_eq!(whoami["address"], json!(namespace), "{whoami:?}");
    assert_eq!(whoami["handle"], json!(null), "{whoami:?}");
    assert_eq!(whoami["contact_policy"], json!("open"), "{whoami:?}");
    assert!(whoami.get("key_hash").is_none(), "{whoami:?}");
    assert!(whoami.get("billing_ref").is_none(), "{whoami:?}");
    assert!(whoami.get("stripe_customer_id").is_none(), "{whoami:?}");
}

#[tokio::test]
async fn ac2_lookup_by_handle_returns_the_claimants_card() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "Agent A").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    client_a
        .tools_call("host.agent.profile_set", json!({"handle": "indexer"}))
        .await
        .expect("A claims @indexer");

    let (_ns_b, key_b) = signup(&server.base_url, "Agent B").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    let raw = client_b
        .tools_call("host.agent.lookup", json!({"address": "@indexer"}))
        .await
        .expect("B looks up @indexer");
    let card = extract_structured(&raw);

    assert_eq!(card["address"], json!(ns_a), "{card:?}");
    assert_eq!(card["handle"], json!("indexer"), "{card:?}");
    for forbidden in ["key_hash", "billing_ref", "stripe_customer_id"] {
        assert!(card.get(forbidden).is_none(), "{card:?}");
    }
}

#[tokio::test]
async fn ac3_claiming_a_taken_handle_fails_without_naming_the_holder() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "Agent A").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    client_a
        .tools_call("host.agent.profile_set", json!({"handle": "indexer"}))
        .await
        .expect("A claims @indexer");

    let (_ns_b, key_b) = signup(&server.base_url, "Agent B").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    // Requirement 3: stored lower-case, so a differently-cased claim still
    // collides.
    let err = client_b
        .tools_call("host.agent.profile_set", json!({"handle": "Indexer"}))
        .await
        .expect_err("B's claim of an already-held handle must fail");
    assert_eq!(err.error_code.as_deref(), Some("handle_taken"));
    let body = serde_json::to_string(&err.data).unwrap_or_default();
    assert!(!body.contains(&ns_a), "error body must not name the holder: {body}");

    let whoami_raw = client_a
        .tools_call("host.agent.whoami", json!({}))
        .await
        .expect("A whoami");
    let whoami = extract_structured(&whoami_raw);
    assert_eq!(whoami["handle"], json!("indexer"), "A's handle must be unchanged: {whoami:?}");
}

#[tokio::test]
async fn ac4_unknown_disabled_and_deleted_are_indistinguishable() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let (_ns_looker, key_looker) = signup(&server.base_url, "Looker").await;
    let looker = McpClient::with_bearer(&server.base_url, &key_looker);

    let (ns_disabled, _key_disabled) = signup(&server.base_url, "Soon Disabled").await;
    admin
        .tools_call("admin.tenant_disable", json!({"tenant": ns_disabled.clone()}))
        .await
        .expect("admin.tenant_disable");

    let (ns_deleted, _key_deleted) = signup(&server.base_url, "Soon Deleted").await;
    admin
        .tools_call("admin.tenant_delete", json!({"tenant": ns_deleted.clone()}))
        .await
        .expect("admin.tenant_delete");

    let err_unknown = looker
        .tools_call("host.agent.lookup", json!({"address": "t_doesnotexist00000000"}))
        .await
        .expect_err("unknown namespace");
    let err_disabled = looker
        .tools_call("host.agent.lookup", json!({"address": ns_disabled}))
        .await
        .expect_err("disabled tenant");
    let err_deleted = looker
        .tools_call("host.agent.lookup", json!({"address": ns_deleted}))
        .await
        .expect_err("deleted tenant");

    for err in [&err_unknown, &err_disabled, &err_deleted] {
        assert_eq!(err.error_code.as_deref(), Some("agent_not_found"), "{err:?}");
    }
    assert_eq!(err_unknown.message, err_disabled.message);
    assert_eq!(err_disabled.message, err_deleted.message);
    assert_eq!(err_unknown.data, err_disabled.data);
    assert_eq!(err_disabled.data, err_deleted.data);
}
