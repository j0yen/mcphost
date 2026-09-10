//! AC1 — Given tenant A's tool `geo` shared `public`, When tenant B calls
//! `t_A.geo` with B's key, Then it succeeds and the run and call rows carry
//! `caller_tenant_id` = B.

mod common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn public_tool_is_callable_by_another_tenant_with_attribution() {
    let server = TestServer::start().await;

    let (ns_a, key_a) = signup(&server.base_url, "Tenant A").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    client_a
        .tools_call(
            "host.tool_publish",
            json!({"name": "geo", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("A publishes geo");
    client_a
        .tools_call(
            "host.tool_share",
            json!({"name": "geo", "visibility": "public", "description": "geo lookup"}),
        )
        .await
        .expect("A shares geo publicly");

    let (ns_b, key_b) = signup(&server.base_url, "Tenant B").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    let qualified = format!("{ns_a}.geo");
    let call = client_b
        .tools_call(&qualified, json!({"msg": "hi"}))
        .await
        .expect("B calls A's public tool");
    let result = extract_structured(&call);
    assert_eq!(result, json!({"msg": "hi"}));

    // Attribution: the `calls` row this produced carries B as the caller,
    // even though it's stored under A's tenant_id (the owner, whose
    // sandbox ran it).
    let tenant_a = server
        .state
        .db
        .find_tenant_by_namespace(ns_a.clone())
        .await
        .expect("find A")
        .expect("A exists");
    let tenant_b = server
        .state
        .db
        .find_tenant_by_namespace(ns_b.clone())
        .await
        .expect("find B")
        .expect("B exists");
    let by_others = server
        .state
        .db
        .calls_by_others(tenant_a.id, 86_400)
        .await
        .expect("calls_by_others");
    assert!(
        by_others.iter().any(|(ns, n)| ns == &ns_b && *n == 1),
        "A's calls_by_others must show B: 1, got {by_others:?}"
    );
    let to_shared = server
        .state
        .db
        .calls_to_shared(tenant_b.id, 86_400)
        .await
        .expect("calls_to_shared");
    assert_eq!(to_shared, 1, "B's calls_to_shared must be 1");
}
