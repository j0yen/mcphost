//! PRD-mcphost-refusals-name-the-nearest-move
//! AC3 (P0) -- Given a tenant with the share `<owner>.deploy_notes`, When it
//! calls `publish_agent_tool` and then `<owner>.deploy_note`, Then the first
//! refusal has `did_you_mean[0]` `host.tool_publish` and `next_call` `{tool:
//! "host.tool_publish", args: <sent>}`, the second has `did_you_mean[0]`
//! `<owner>.deploy_notes` and `next_call.tool` `host.tool.list_shared`, and
//! unknown-owner, unknown-tool and unshared misses are byte-identical.

use crate::common::{McpClient, RpcError, TestServer, signup};
use mcphost::auth::{generate_key, hash_key};
use serde_json::{Value, json};

/// The refusal as a client sees it, minus the per-call `request_id`.
fn shape(err: &RpcError) -> Value {
    let mut data = err.data.clone();
    data.as_object_mut().expect("data is an object").remove("request_id");
    json!({"code": err.code, "error_code": err.error_code, "message": err.message, "data": data})
}

#[tokio::test]
async fn nmove_ac03() {
    let server = TestServer::start().await;
    let echo = json!({"schema": {"type": "object"}});

    // O shares `deploy_notes` with B through a group.
    let (ns_o, key_o) = signup(&server.base_url, "Nmove03 Owner").await;
    let owner = McpClient::with_bearer(&server.base_url, &key_o);
    owner
        .tools_call("host.tool_publish", json!({"name": "deploy_notes", "kind": "echo", "spec": echo}))
        .await
        .expect("O publishes deploy_notes");
    owner.tools_call("host.group.create", json!({"name": "team"})).await.expect("group");
    let (ns_b, key_b) = signup(&server.base_url, "Nmove03 B").await;
    let b = McpClient::with_bearer(&server.base_url, &key_b);
    owner.tools_call("host.group.add", json!({"name": "team", "namespace": ns_b})).await.expect("add B");
    owner
        .tools_call("host.tool_share", json!({"name": "deploy_notes", "visibility": "group", "group": "team"}))
        .await
        .expect("share to team");

    // A control-plane verb used as a tool name: the refusal hands back that verb with the args sent.
    let sent = json!({"name": "x", "kind": "echo", "spec": {"schema": {"type": "object"}}});
    let err = b.tools_call("publish_agent_tool", sent.clone()).await.expect_err("not a tool");
    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"), "{err:?}");
    assert_eq!(err.data["did_you_mean"][0], "host.tool_publish", "{err:?}");
    assert_eq!(err.data["next_call"], json!({"tool": "host.tool_publish", "args": sent}), "{err:?}");

    // A mistyped shared name: the owner's real shared name and the share listing.
    let err = b.tools_call(&format!("{ns_o}.deploy_note"), json!({})).await.expect_err("mistyped share");
    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"), "{err:?}");
    assert_eq!(err.message, format!("tool not found: {ns_o}.deploy_note"));
    assert_eq!(err.data["did_you_mean"][0], format!("{ns_o}.deploy_notes"), "{err:?}");
    assert_eq!(err.data["next_call"]["tool"], "host.tool.list_shared", "{err:?}");
    assert_eq!(err.data["hint"], mcphost::errors::SHARED_TOOL_NOT_FOUND_HINT, "the shared hint stays: {err:?}");

    // The same literal name, three backing states: no such owner, an owner with no such tool,
    // an owner whose tool is private. The refusals are byte-identical.
    let name = "acme.lookup";
    let unknown_owner = shape(&b.tools_call(name, json!({})).await.expect_err("no acme yet"));
    let acme_key = generate_key();
    server
        .state
        .db
        .create_tenant("Acme Corp".to_string(), "acme".to_string(), hash_key(&acme_key), None)
        .await
        .expect("create acme");
    let unknown_tool = shape(&b.tools_call(name, json!({})).await.expect_err("acme has no lookup"));
    McpClient::with_bearer(&server.base_url, &acme_key)
        .tools_call("host.tool_publish", json!({"name": "lookup", "kind": "echo", "spec": echo}))
        .await
        .expect("acme publishes lookup, left private");
    let unshared = shape(&b.tools_call(name, json!({})).await.expect_err("lookup is private"));
    assert_eq!(unknown_owner, unknown_tool);
    assert_eq!(unknown_tool, unshared);
    assert_eq!(unshared["data"]["tools_published"], 0);
    assert!(unshared["data"]["nearest"].as_array().is_some_and(|n| !n.is_empty()));
}
