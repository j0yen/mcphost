//! AC5 — Given a shared tool at version 3 and a caller pinning
//! `version: 2`, When the owner publishes version 4, Then the pinned call
//! still runs version 2.
//!
//! Cross-tenant calls have no `{name, args}` wrapper the way
//! `host.tool_call` does -- pinning rides a reserved top-level `version`
//! field inside the call's own arguments object, stripped before it
//! reaches the tool's args_schema (see `handler::call_tool`'s cross-tenant
//! arm). Each version's schema requires a distinct `tag` const, so a
//! successful call with `{"tag": "v2", "version": 2}` is only possible if
//! version 2's spec is what actually ran.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

fn tagged_schema(tag: &str) -> serde_json::Value {
    json!({
        "schema": {
            "type": "object",
            "properties": {"tag": {"const": tag}},
            "required": ["tag"],
            "additionalProperties": false,
        }
    })
}

#[tokio::test]
async fn pinned_cross_tenant_call_ignores_a_later_republish() {
    let server = TestServer::start().await;

    let (ns_a, key_a) = signup(&server.base_url, "Tenant A").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    for tag in ["v1", "v2", "v3"] {
        client_a
            .tools_call(
                "host.tool_publish",
                json!({"name": "greet", "kind": "echo", "spec": tagged_schema(tag)}),
            )
            .await
            .unwrap_or_else(|e| panic!("A publishes {tag}: {} {}", e.code, e.message));
    }
    client_a
        .tools_call("host.tool_share", json!({"name": "greet", "visibility": "public"}))
        .await
        .expect("A shares greet publicly");

    let (_ns_b, key_b) = signup(&server.base_url, "Tenant B").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    let qualified = format!("{ns_a}.greet");

    let pinned = client_b
        .tools_call(&qualified, json!({"tag": "v2", "version": 2}))
        .await
        .expect("B's pinned call to version 2 succeeds while current is 3");
    assert_eq!(extract_structured(&pinned), json!({"tag": "v2"}));

    // Owner republishes to version 4 -- current moves, but B's pin doesn't.
    client_a
        .tools_call(
            "host.tool_publish",
            json!({"name": "greet", "kind": "echo", "spec": tagged_schema("v4")}),
        )
        .await
        .expect("A publishes v4");

    let still_pinned = client_b
        .tools_call(&qualified, json!({"tag": "v2", "version": 2}))
        .await
        .expect("B's pin to version 2 still succeeds after A publishes v4");
    assert_eq!(extract_structured(&still_pinned), json!({"tag": "v2"}));
}
