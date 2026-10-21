//! PRD-mcphost-invalid-spec-error-carries-kind-fields
//! AC3 (P0) — Given the same invalid spec sent to `host.spec.test` and the
//! tool-test verb that takes a raw spec (`host.bridge.test`; `host.tool.test`
//! only takes a published tool's name), When each validates, Then the refusal
//! `data` equals the publish refusal `data` byte for byte.

use crate::common::{McpClient, TestServer, http_kind_registry, signup};
use serde_json::{Value, json};

/// The keys `invalid_spec` adds; the envelope's per-response keys
/// (`request_id`, `help_url`, ...) are not part of the payload under test.
const PAYLOAD_KEYS: &[&str] = &["kind", "field", "reason", "fields", "minimal_specs", "docs_url"];

fn payload(data: &Value) -> String {
    let picked: serde_json::Map<String, Value> =
        PAYLOAD_KEYS.iter().map(|k| (k.to_string(), data[*k].clone())).collect();
    serde_json::to_string(&Value::Object(picked)).unwrap()
}

#[tokio::test]
async fn spkind_ac03_same_data_across_verbs() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Spkind03").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let spec = json!({"url": "https://example.com"});

    let publish = client
        .tools_call("host.tool.publish", json!({"name": "spkind_ac03", "kind": "http", "spec": spec}))
        .await
        .expect_err("publish refuses");
    let spec_test = client
        .tools_call("host.spec.test", json!({"kind": "http", "spec": spec}))
        .await
        .expect_err("spec.test refuses");
    let bridge_test = client
        .tools_call("host.bridge.test", json!({"spec": spec, "args": {}}))
        .await
        .expect_err("bridge.test refuses");

    for err in [&publish, &spec_test, &bridge_test] {
        assert_eq!(err.error_code.as_deref(), Some("invalid_spec"), "{err:?}");
    }
    let want = payload(&publish.data);
    assert!(publish.data["minimal_specs"].as_array().is_some_and(|l| !l.is_empty()), "{want}");
    assert_eq!(payload(&spec_test.data), want, "host.spec.test");
    assert_eq!(payload(&bridge_test.data), want, "host.bridge.test");
}
