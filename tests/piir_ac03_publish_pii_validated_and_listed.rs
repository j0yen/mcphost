//! PRD-mcphost-pii-tagging-and-log-redaction-v2
//! AC3 -- Given a tool schema with `phone` and without `nope`, When
//! `host.tool.publish` runs with `pii: ["nope"]`, Then `args_invalid` names it;
//! with `"x-pii": true` on `phone`, Then `host.tool.list` shows
//! `pii_args: ["phone"]`; Given a republish without `pii`, Then the list is
//! kept, and with `pii: []` it is cleared.

use crate::common;
use common::{McpClient, RpcError, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::{Value, json};

fn spec(phone_schema: Value) -> Value {
    json!({
        "method": "POST",
        "url": "http://127.0.0.1:9/echo",
        "args_schema": {"type": "object", "properties": {"phone": phone_schema, "note": {"type": "string"}}},
    })
}

async fn publish(client: &McpClient, spec: Value, pii: Option<Value>) -> Result<Value, RpcError> {
    let mut args = json!({"name": "dialer", "kind": "http", "spec": spec});
    if let Some(pii) = pii {
        args["pii"] = pii;
    }
    client.tools_call("host.tool_publish", args).await.map(|v| extract_structured(&v))
}

async fn pii_args(client: &McpClient) -> Value {
    let listed = extract_structured(&client.tools_call("host.tool_list", json!({})).await.expect("host.tool_list"));
    let row = listed["tools"]
        .as_array()
        .and_then(|rows| rows.iter().find(|r| r["name"].as_str().is_some_and(|n| n.ends_with(".dialer"))))
        .unwrap_or_else(|| panic!("dialer is listed: {listed}"));
    row["pii_args"].clone()
}

#[tokio::test]
async fn piir_ac03_publish_pii_validated_and_listed() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "piir ac03 tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let plain = json!({"type": "string"});

    // A name the schema lacks is refused, naming it, and nothing is stored.
    let refused = publish(&client, spec(plain.clone()), Some(json!(["nope"]))).await.expect_err("nope is not a property");
    assert_eq!(refused.error_code.as_deref(), Some("args_invalid"), "{refused:?}");
    assert!(refused.message.contains("nope"), "the refusal names the argument: {refused:?}");
    let listed = extract_structured(&client.tools_call("host.tool_list", json!({})).await.expect("list"));
    assert!(listed["tools"].as_array().is_none_or(Vec::is_empty), "a refused publish stores nothing: {listed}");

    // A malformed `pii` is refused too.
    let bad = publish(&client, spec(plain.clone()), Some(json!("phone"))).await.expect_err("pii must be an array");
    assert_eq!(bad.error_code.as_deref(), Some("args_invalid"), "{bad:?}");

    // `x-pii: true` on a property tags it without a `pii` argument.
    publish(&client, spec(json!({"type": "string", "x-pii": true})), None).await.expect("publish with x-pii");
    assert_eq!(pii_args(&client).await, json!(["phone"]), "x-pii joins the set");

    // A republish without `pii` keeps the stored list, even where the schema no longer says x-pii.
    publish(&client, spec(plain.clone()), None).await.expect("republish without pii");
    assert_eq!(pii_args(&client).await, json!(["phone"]), "the list is kept");

    // ... and drops a name the new schema lacks.
    publish(
        &client,
        json!({"method": "POST", "url": "http://127.0.0.1:9/echo", "args_schema": {"type": "object", "properties": {"note": plain.clone()}}}),
        None,
    )
    .await
    .expect("republish without phone");
    assert_eq!(pii_args(&client).await, json!([]), "a name the schema lacks is dropped");

    // An explicit `pii` tags; `pii: []` clears.
    publish(&client, spec(plain.clone()), Some(json!(["phone", "phone"]))).await.expect("explicit pii");
    assert_eq!(pii_args(&client).await, json!(["phone"]), "explicit names, deduped");
    publish(&client, spec(plain.clone()), Some(json!([]))).await.expect("pii: []");
    assert_eq!(pii_args(&client).await, json!([]), "pii: [] clears the list");

    // An untagged tool lists an empty array, never a missing key.
    let listed = extract_structured(&client.tools_call("host.tool_list", json!({})).await.expect("list"));
    assert_eq!(listed["tools"][0]["pii_args"], json!([]), "{listed}");
}
