//! PRD-mcphost-session-key
//! AC5 — Given a key returned by `signup`, When a client with no
//! `Authorization` header calls `host.whoami` with that key as
//! `tenant_key`, Then it receives the same payload the bearer-authenticated
//! path returns for that tenant.
//! AC7 — Given a client sending both `Authorization: Bearer <key-of-tenant-A>`
//! and a `tenant_key` argument for tenant B, When it calls `host.whoami`,
//! Then the response describes tenant A, proving the header takes
//! precedence.
//! AC8 — Given a `tenant_key` that matches no tenant, When it is passed to
//! `host.tool_list`, Then the call fails the same way an unrecognised
//! bearer token does functionally (refused, no tenant data) -- though
//! PRD-mcphost-auth-error-names-argument now gives the two paths distinct
//! codes (`tenant_key_invalid` vs `bearer_invalid`) and the argument
//! path's message never mentions the Authorization header.
//! AC9 — Given a tenant disabled by `admin.tenant_disable`, When its key is
//! passed as `tenant_key`, Then the call is refused with the
//! tenant-disabled error, matching the bearer path.

mod common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn whoami_via_tenant_key_matches_the_bearer_path() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Argument-Auth Tenant").await;

    let bearer_client = McpClient::with_bearer(&server.base_url, &key);
    let bearer_result = extract_structured(
        &bearer_client
            .tools_call("host.whoami", json!({}))
            .await
            .expect("bearer whoami"),
    );

    let anon_client = McpClient::new(&server.base_url);
    let arg_result = extract_structured(
        &anon_client
            .tools_call("host.whoami", json!({"tenant_key": key}))
            .await
            .expect("tenant_key whoami"),
    );

    assert_eq!(
        bearer_result, arg_result,
        "an argument-authenticated whoami must return exactly what the bearer path returns"
    );
}

#[tokio::test]
async fn header_takes_precedence_over_a_tenant_key_argument() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "Tenant A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Tenant B").await;
    assert_ne!(ns_a, ns_b);

    let client = McpClient::with_bearer(&server.base_url, &key_a);
    let result = extract_structured(
        &client
            .tools_call("host.whoami", json!({"tenant_key": key_b}))
            .await
            .expect("whoami with conflicting header/argument"),
    );

    assert_eq!(
        result["tenant"].as_str(),
        Some(ns_a.as_str()),
        "the header's tenant (A) must win over the tenant_key argument's tenant (B): {result}"
    );
}

#[tokio::test]
async fn unrecognised_tenant_key_is_refused_but_distinct_from_a_bad_bearer() {
    let server = TestServer::start().await;

    let bad_bearer_client = McpClient::with_bearer(&server.base_url, "not-a-real-key");
    let bearer_err = bad_bearer_client
        .tools_call("host.tool_list", json!({}))
        .await
        .expect_err("an invalid bearer must be refused");

    let anon_client = McpClient::new(&server.base_url);
    let arg_err = anon_client
        .tools_call("host.tool_list", json!({"tenant_key": "not-a-real-key"}))
        .await
        .expect_err("an unrecognised tenant_key must be refused too");

    // PRD-mcphost-auth-error-names-argument requirement 3 / AC4: the
    // header path keeps its own code and text, unchanged.
    assert_eq!(bearer_err.error_code.as_deref(), Some("bearer_invalid"));
    assert_eq!(bearer_err.message, "missing or invalid Authorization: Bearer key");
    // Requirement 2 / AC2: the argument path gets its own code and a
    // message that neither echoes the key nor mentions the header.
    assert_eq!(arg_err.error_code.as_deref(), Some("tenant_key_invalid"));
    assert!(!arg_err.message.contains("not-a-real-key"));
    assert!(!arg_err.message.contains("Authorization"));
    assert_ne!(
        bearer_err.message, arg_err.message,
        "the two paths must now read differently -- one names a header the caller can't send"
    );
}

#[tokio::test]
async fn disabled_tenants_key_is_refused_as_a_tenant_key_argument_too() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "About To Be Disabled").await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    admin
        .tools_call("admin.tenant_disable", json!({"tenant": ns}))
        .await
        .expect("admin.tenant_disable");

    let anon_client = McpClient::new(&server.base_url);
    let err = anon_client
        .tools_call("host.whoami", json!({"tenant_key": key}))
        .await
        .expect_err("a disabled tenant's key must be refused even as an argument");
    // PRD-mcphost-auth-error-names-argument requirement 2 / AC3: the
    // tenant_key argument path folds "disabled" into tenant_key_invalid --
    // it never confirms the key belonged to a real (if disabled) tenant.
    // The header path's own tenant_disabled code is unchanged (see
    // ac08_admin_disable_and_forbidden.rs).
    assert_eq!(err.error_code.as_deref(), Some("tenant_key_invalid"));
}
