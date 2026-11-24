//! PRD-mcphost-invalid-spec-message-carries-fix
//! AC3 (P0) — Given the same invalid spec sent to `host.tool.publish`,
//! `host.spec.test` and `host.bridge.test`, When each refuses, Then the three
//! `message` strings are byte-equal and the three `data` values are byte-equal on
//! the keys `kind`, `field`, `reason`, `fields`, `minimal_specs`, `docs_url`.

use crate::common::{McpClient, TestServer, http_kind_registry, signup};
use serde_json::{Value, json};

const PAYLOAD_KEYS: &[&str] = &["kind", "field", "reason", "fields", "minimal_specs", "docs_url"];

fn payload(data: &Value) -> String {
    let picked: serde_json::Map<String, Value> =
        PAYLOAD_KEYS.iter().map(|k| (k.to_string(), data[*k].clone())).collect();
    serde_json::to_string(&Value::Object(picked)).unwrap()
}

#[tokio::test]
async fn spmsg_ac03() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Spmsg03").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let spec = json!({"url": "https://example.com"});

    let publish = client
        .tools_call("host.tool.publish", json!({"name": "spmsg_ac03", "kind": "http", "spec": spec}))
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
    assert!(publish.message.contains("; minimal spec: "), "{}", publish.message);
    assert_eq!(spec_test.message, publish.message, "host.spec.test message");
    assert_eq!(bridge_test.message, publish.message, "host.bridge.test message");
    let want = payload(&publish.data);
    assert_eq!(payload(&spec_test.data), want, "host.spec.test data");
    assert_eq!(payload(&bridge_test.data), want, "host.bridge.test data");
}
