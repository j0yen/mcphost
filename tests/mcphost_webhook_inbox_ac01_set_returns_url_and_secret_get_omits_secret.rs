//! PRD-mcphost-webhook-inbox
//! AC1 (P0) — Given `host.trigger.set kind=webhook name=pay tool=on_payment`,
//! When it returns, Then the response has `url` and `secret`, and
//! `host.trigger.get` afterwards has the url and no secret.

use crate::common;
use common::{extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn webhook_set_returns_secret_once_get_never_repeats_it() {
    let server = common::TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC1 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "on_payment", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");

    let set = extract_structured(
        &client
            .tools_call(
                "host.trigger.set",
                json!({"tool": "on_payment", "kind": "webhook", "name": "pay"}),
            )
            .await
            .expect("trigger.set"),
    );
    let url = set["url"].as_str().expect("url present").to_string();
    let secret = set["secret"].as_str().expect("secret present").to_string();
    assert!(!secret.is_empty());
    assert!(url.starts_with(&server.base_url));
    assert!(url.contains("/hook/"));
    let trigger_id = set["id"].as_str().expect("id").to_string();

    let got = extract_structured(
        &client
            .tools_call("host.trigger.get", json!({"id": trigger_id}))
            .await
            .expect("trigger.get"),
    );
    assert_eq!(got["url"], json!(url), "host.trigger.get must still carry the url");
    assert!(
        got.get("secret").is_none() || got["secret"].is_null(),
        "host.trigger.get must never repeat the secret: {got:?}"
    );

    // host.trigger.list must agree.
    let listed = extract_structured(
        &client
            .tools_call("host.trigger.list", json!({}))
            .await
            .expect("trigger.list"),
    );
    let triggers = listed["triggers"].as_array().expect("triggers array");
    let listed_one = triggers
        .iter()
        .find(|t| t["id"] == json!(trigger_id))
        .expect("the webhook trigger is listed");
    assert_eq!(listed_one["url"], json!(url));
    assert!(
        listed_one.get("secret").is_none() || listed_one["secret"].is_null(),
        "host.trigger.list must never carry the secret: {listed_one:?}"
    );
}
