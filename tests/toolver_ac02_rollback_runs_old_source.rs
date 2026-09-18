//! AC2 — Given version 2 current, When `host.tool_rollback {version: 1}`
//! is called, Then the next `host.tool_call` runs version 1's source.
//!
//! Uses two `echo` specs whose schemas require a distinct `const` tag
//! (`v1`/`v2`) so which version actually ran is provable without a
//! sandboxed kind: calling with the OTHER version's tag must fail
//! `args_invalid` because that version's schema no longer accepts it.

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
async fn rollback_makes_the_next_call_run_the_old_spec() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Publisher").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "greet", "kind": "echo", "spec": tagged_schema("v1")}),
        )
        .await
        .expect("publish v1");
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "greet", "kind": "echo", "spec": tagged_schema("v2")}),
        )
        .await
        .expect("publish v2");

    // Current is v2: a v2-tagged call succeeds, a v1-tagged one fails.
    let call = client
        .tools_call("host.tool_call", json!({"name": "greet", "args": {"tag": "v2"}}))
        .await
        .expect("v2 call succeeds while v2 is current");
    assert_eq!(extract_structured(&call), json!({"tag": "v2"}));
    let err = client
        .tools_call("host.tool_call", json!({"name": "greet", "args": {"tag": "v1"}}))
        .await
        .unwrap_err();
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"));

    let rollback = client
        .tools_call("host.tool_rollback", json!({"name": "greet", "version": 1}))
        .await
        .expect("rollback to v1");
    assert_eq!(extract_structured(&rollback)["current_version"], json!(1));

    // Current is now v1: the v1-tagged call succeeds, the v2-tagged one
    // fails -- proving the ROLLED-BACK source is what actually ran, not
    // just that the pointer moved.
    let call = client
        .tools_call("host.tool_call", json!({"name": "greet", "args": {"tag": "v1"}}))
        .await
        .expect("v1 call succeeds after rollback");
    assert_eq!(extract_structured(&call), json!({"tag": "v1"}));
    let err = client
        .tools_call("host.tool_call", json!({"name": "greet", "args": {"tag": "v2"}}))
        .await
        .unwrap_err();
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"));
}
