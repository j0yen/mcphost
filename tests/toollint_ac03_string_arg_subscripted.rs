//! PRD-mcphost-publish-time-tool-lint
//! AC3 (P0) -- Given `payload = args["payload"]` on line 12 and
//! `payload["id"]` on line 16 with `payload` inferred as `string` (default
//! `""`), When published, Then one `string_arg_subscripted` finding at line 16
//! names `payload`, `json.loads`, and `object`; Given `payload` typed `object`
//! in a declared `args_schema`, Then no finding.

use crate::common;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use serde_json::{Value, json};

/// `bind` is the line-12 statement; line 16 subscripts `payload`.
fn source(bind: &str) -> String {
    format!(
        "import json\n\n\n# unpack the payload\n\n\n\ndef main(args):\n    unused = 1\n    # note\n    pass\n    {bind}\n    # parse below\n\n    pass\n    ident = payload[\"id\"]\n    return {{\"id\": ident}}\n"
    )
}

async fn publish(client: &McpClient, name: &str, spec: Value) -> Value {
    let result = client
        .tools_call("host.tool_publish", json!({"name": name, "kind": "python", "spec": spec}))
        .await
        .unwrap_or_else(|e| panic!("publish must succeed despite a lint: {} {}", e.code, e.message));
    extract_structured(&result)
}

#[tokio::test]
async fn string_typed_argument_subscripted_reports_line_and_fix() {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Toollint AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // Inferred: the default "" types `payload` as string.
    let inferred = source("payload = args.get(\"payload\", \"\")");
    assert_eq!(inferred.lines().nth(11).unwrap().trim(), "payload = args.get(\"payload\", \"\")");
    assert_eq!(inferred.lines().nth(15).unwrap().trim(), "ident = payload[\"id\"]");
    let published = publish(&client, "inferred", json!({"source": inferred})).await;
    let lint = published["tool_lint"].as_array().expect("tool_lint is an array");
    assert_eq!(lint.len(), 1, "exactly one finding: {lint:?}");
    assert_eq!(lint[0]["rule"], json!("string_arg_subscripted"));
    assert_eq!(lint[0]["line"], json!(16));
    let text = format!("{} {}", lint[0]["message"].as_str().unwrap(), lint[0]["fix"].as_str().unwrap());
    for needle in ["payload", "json.loads", "object"] {
        assert!(text.contains(needle), "finding must name {needle}: {text}");
    }

    // Declared string schema + the `args["payload"]` spelling is the same shape.
    let declared_string = source("payload = args[\"payload\"]");
    let published = publish(
        &client,
        "declared_string",
        json!({"source": declared_string, "args_schema": {"type": "object", "properties": {"payload": {"type": "string"}}}}),
    )
    .await;
    assert_eq!(published["tool_lint"][0]["line"], json!(16), "{published}");

    // Declared object: no finding.
    let published = publish(
        &client,
        "declared_object",
        json!({"source": declared_string, "args_schema": {"type": "object", "properties": {"payload": {"type": "object"}}}}),
    )
    .await;
    assert_eq!(published["tool_lint"], json!([]), "an object-typed argument lints clean: {published}");
}
