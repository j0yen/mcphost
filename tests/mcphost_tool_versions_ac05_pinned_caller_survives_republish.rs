//! PRD-mcphost-tool-versions
//! AC5 (P0) — Given a shared tool at version 3 and a caller pinning
//! `version: 2`, When the owner publishes version 4, Then the pinned call
//! still runs version 2.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

fn spec_v(n: u32) -> serde_json::Value {
    json!({"schema": {"type": "object", "properties": {"f": {"type": "string", "enum": [format!("v{n}")]}}, "required": ["f"]}})
}

#[tokio::test]
async fn pinned_cross_tenant_call_survives_owners_republish() {
    let server = TestServer::start().await;

    let (ns_a, key_a) = signup(&server.base_url, "AC5 Owner").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    for n in 1..=3 {
        client_a
            .tools_call(
                "host.tool_publish",
                json!({"name": "shared", "kind": "echo", "spec": spec_v(n)}),
            )
            .await
            .unwrap_or_else(|e| panic!("publish v{n} should succeed: {} {}", e.code, e.message));
    }
    client_a
        .tools_call(
            "host.tool_share",
            json!({"name": "shared", "visibility": "public", "description": "AC5"}),
        )
        .await
        .expect("share should succeed");

    let (_ns_b, key_b) = signup(&server.base_url, "AC5 Caller").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    let qualified = format!("{ns_a}.shared");

    // Pin to version 2 while v3 is current.
    let pinned = client_b
        .tools_call(&qualified, json!({"f": "v2", "version": 2}))
        .await
        .expect("pinned call to version 2 should succeed");
    assert_eq!(extract_structured(&pinned), json!({"f": "v2"}));

    // The owner republishes to v4.
    client_a
        .tools_call(
            "host.tool_publish",
            json!({"name": "shared", "kind": "echo", "spec": spec_v(4)}),
        )
        .await
        .expect("publish v4 should succeed");

    // A pinned call to version 2 must still run version 2's schema -- args
    // shaped for v4 must now be rejected under the pin.
    let still_v2 = client_b
        .tools_call(&qualified, json!({"f": "v2", "version": 2}))
        .await
        .expect("pinned call to version 2 must still succeed after v4 ships");
    assert_eq!(extract_structured(&still_v2), json!({"f": "v2"}));

    let rejected = client_b.tools_call(&qualified, json!({"f": "v4", "version": 2})).await;
    assert!(
        rejected.is_err(),
        "v4-shaped args must be rejected while pinned to version 2"
    );
}
