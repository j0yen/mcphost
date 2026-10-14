//! PRD-mcphost-schema-coercion-opt-in AC5 -- Given `x-coerce` on a property
//! inside `items` of an array and on a property two levels deep, When
//! called, Then the first is coerced and the second is left untouched with
//! no error. (`4.0` is a valid `integer` to jsonschema, so an untouched
//! value passes validation; a coerced one would become the integer 4.)

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::kinds::KindRegistry;
use serde_json::json;

#[tokio::test]
async fn items_property_is_coerced_and_two_levels_deep_is_untouched() {
    let server = TestServer::start_with_kinds(KindRegistry::with_builtin()).await;
    let (ns, key) = signup(&server.base_url, "Coerce AC5").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let n = json!({"type": "integer", "x-coerce": true});
    let schema = json!({
        "type": "object",
        "properties": {
            "rows": {"type": "array", "items": {"type": "object", "properties": {"n": n}}},
            "deep": {"type": "object", "properties": {"mid": {"type": "object", "properties": {"n": n}}}},
        }
    });
    client
        .tools_call("host.tool_publish", json!({"name": "nested", "kind": "echo", "spec": {"schema": schema}}))
        .await
        .expect("publish");

    let call = client
        .tools_call(&format!("{ns}.nested"), json!({"rows": [{"n": "4"}, {"n": 5.0}], "deep": {"mid": {"n": 6.0}}}))
        .await
        .expect("no error");
    let r = extract_structured(&call);
    assert!(r["rows"][0]["n"].is_i64() && r["rows"][0]["n"] == json!(4), "first coerced: {r}");
    assert!(r["rows"][1]["n"].is_i64(), "second item coerced too: {r}");
    assert!(r["deep"]["mid"]["n"].is_f64(), "two levels deep untouched (still 6.0): {r}");
    assert_eq!(
        r["coerced"].as_array().map(Vec::len),
        Some(2),
        "only the two items coerced, nothing at depth 2: {r}"
    );
    assert_eq!(r["coerced"][0]["instance_path"], json!("/rows/0/n"));
}
