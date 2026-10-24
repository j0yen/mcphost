//! PRD-mcphost-named-key-refusal-not-http-401
//! AC3 -- a connection that already named tenant T, then a keyless
//! host.tool_publish: HTTP 200, -32602 `tenant_key_missing`, `data.reason`
//! connection_bound, `data.tenant` T, the R3 connection hint, and a
//! `data.get_a_key` naming signup and host.quickstart. Every serialized
//! refusal's `data` keys stay within `REFUSAL_FIELDS`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, parse_response_body};
use mcphost::errors::{GET_A_KEY, REFUSAL_FIELDS, connection_hint};
use serde_json::json;

fn publish(name: &str) -> serde_json::Value {
    json!({"name": name, "kind": "echo", "spec": {"schema": {"type": "object", "properties": {}, "required": []}}})
}

#[tokio::test]
async fn nk401_ac03() {
    let server = TestServer::start().await;
    let session = McpClient::new(&server.base_url).with_session_continuity();
    let first = extract_structured(&session.tools_call("host.tool_publish", publish("hello")).await.expect("first"));
    let ns = first["onboarding"]["tenant"].as_str().expect("tenant").to_string();

    let resp = session
        .post_with_mcp_name_override(
            json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call",
                   "params": {"name": "host.tool_publish", "arguments": publish("second")}}),
            "host.tool_publish",
        )
        .await;
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    assert!(resp.headers().get(reqwest::header::WWW_AUTHENTICATE).is_none());
    let body = parse_response_body(resp).await;
    assert_eq!(body["error"]["code"], json!(-32602), "{body}");
    let data = &body["error"]["data"];
    assert_eq!(data["error_code"], json!("tenant_key_missing"), "{body}");
    assert_eq!(data["reason"], json!("connection_bound"), "{body}");
    assert_eq!(data["tenant"], json!(ns), "{body}");
    assert_eq!(data["hint"], json!(connection_hint(&ns)), "{body}");
    let get_a_key = data[GET_A_KEY].as_str().unwrap_or_else(|| panic!("get_a_key: {body}"));
    assert!(get_a_key.contains("signup") && get_a_key.contains("host.quickstart"), "{get_a_key}");
    for key in data.as_object().expect("data object").keys() {
        assert!(REFUSAL_FIELDS.contains(&key.as_str()), "unlisted refusal field {key}: {body}");
    }
}
