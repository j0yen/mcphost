//! AC4 (P0) — Given `host.trigger.set` called with an argument key the
//! tool does not define, When dispatched, Then the error class is
//! `unknown_argument` with `data.known` equal to the registered properties
//! and no trigger is created.
//!
//! `host.trigger.set` is the PRD's own example tool (Grounding: a `name`
//! argument was once silently dropped on it) -- this uses a key no PRD has
//! ever defined (`bogus_field`) rather than `name` itself, since `name` is
//! in fact already a registered property of this tool (the webhook-inbox
//! table name), not the unknown-argument case this AC means.

use crate::common;
use common::{TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn an_unrecognized_argument_is_refused_naming_the_registered_properties() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Unkfield AC4").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // The tool's own registered property set, read straight from
    // tools/list -- so this test can't drift from whatever host.trigger.set
    // actually declares, in either direction.
    let tools_list = client.tools_list().await.expect("tools/list");
    let tools = tools_list["tools"].as_array().expect("tools array");
    let trigger_set = tools
        .iter()
        .find(|t| t["name"] == json!("host.trigger.set"))
        .expect("host.trigger.set is registered");
    let mut expected_known: Vec<String> = trigger_set["inputSchema"]["properties"]
        .as_object()
        .expect("inputSchema.properties")
        .keys()
        .cloned()
        .collect();
    expected_known.sort();
    assert_eq!(
        trigger_set["inputSchema"]["additionalProperties"],
        json!(false),
        "AC5 should already have made this schema additionalProperties: false"
    );

    let err = client
        .tools_call(
            "host.trigger.set",
            json!({"tool": "whatever", "bogus_field": true}),
        )
        .await
        .expect_err("an argument the tool never declared must be refused");

    assert_eq!(err.error_code.as_deref(), Some("unknown_argument"));
    assert_eq!(err.data["argument"], json!("bogus_field"));
    let mut got_known: Vec<String> = err.data["known"]
        .as_array()
        .expect("known is an array")
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    got_known.sort();
    assert_eq!(got_known, expected_known, "data.known must equal the registered properties");

    // No trigger was created -- the bogus call never reached
    // `triggers::set` at all.
    let list = client
        .tools_call("host.trigger.list", json!({}))
        .await
        .expect("host.trigger.list");
    let triggers = common::extract_structured(&list)["triggers"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(triggers.is_empty(), "a refused call must not create a trigger row");
}

/// A call carrying only recognized arguments is unaffected by this check
/// (it still fails downstream, on `tool` naming no published tool -- this
/// test only proves the unknown-argument gate isn't firing where it
/// shouldn't).
#[tokio::test]
async fn a_call_with_only_known_arguments_is_not_refused_as_unknown_argument() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Unkfield AC4b").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call("host.trigger.set", json!({"tool": "does_not_exist"}))
        .await
        .expect_err("naming an unpublished tool must still fail, just not as unknown_argument");

    assert_ne!(err.error_code.as_deref(), Some("unknown_argument"));
}
