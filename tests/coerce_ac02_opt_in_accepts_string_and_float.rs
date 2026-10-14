//! PRD-mcphost-schema-coercion-opt-in AC2 (+ R5) -- Given
//! `{"n": {"type": "integer", "x-coerce": true}}`, When called with `"4"`
//! and with `4.0`, Then both run with `n` equal to JSON integer 4 and
//! `coerced` lists the pair; `x-coerce` survives publish -> tools/list.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::kinds::KindRegistry;
use serde_json::json;

pub async fn publish_n(client: &McpClient, name: &str, schema: serde_json::Value) {
    client
        .tools_call("host.tool_publish", json!({"name": name, "kind": "echo", "spec": {"schema": schema}}))
        .await
        .expect("publish");
}

#[tokio::test]
async fn string_and_float_run_as_integer_and_are_reported() {
    let server = TestServer::start_with_kinds(KindRegistry::with_builtin()).await;
    let (ns, key) = signup(&server.base_url, "Coerce AC2").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let schema = json!({"type": "object", "properties": {"n": {"type": "integer", "x-coerce": true}}});
    publish_n(&client, "opt_in", schema.clone()).await;

    for (sent, from) in [(json!("4"), "string"), (json!(4.0), "number")] {
        let call = client.tools_call(&format!("{ns}.opt_in"), json!({"n": sent})).await.expect("coerced call runs");
        let result = extract_structured(&call);
        assert_eq!(result["n"].as_i64(), Some(4), "n must be JSON integer 4: {result}");
        assert!(result["n"].is_i64(), "{result}");
        assert_eq!(
            result["coerced"],
            json!([{"argument": "n", "instance_path": "/n", "from": from, "to": "integer"}])
        );
    }

    // R5: the keyword is preserved verbatim in tools/list.
    let tools = client.tools_list().await.expect("tools/list");
    let tool = tools["tools"].as_array().unwrap().iter().find(|t| t["name"] == format!("{ns}.opt_in")).expect("listed");
    assert_eq!(tool["inputSchema"]["properties"]["n"]["x-coerce"], json!(true), "{tool}");

    // An already-correct call carries no `coerced` key.
    let plain = extract_structured(&client.tools_call(&format!("{ns}.opt_in"), json!({"n": 4})).await.unwrap());
    assert!(plain.get("coerced").is_none(), "{plain}");
}
