//! PRD-mcphost-invalid-spec-error-carries-kind-fields
//! AC6 (P1) — Given a spec referencing secret `spkind_missing`, When
//! published by a tenant holding secrets `a` and `b`, Then the refusal
//! `data.fix` is `{tool: "host.secret.set", args: {name: "spkind_missing"}}`
//! and `known_secret_names` is `["a","b"]` with no values anywhere in the
//! envelope.

use crate::common::{McpClient, TestServer, http_kind_registry, signup};
use serde_json::json;

#[tokio::test]
async fn spkind_ac06_unknown_secret_names_the_fix() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Spkind06").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    for (name, value) in [("a", "spkind-value-of-a"), ("b", "spkind-value-of-b")] {
        client
            .tools_call("host.secret_set", json!({"name": name, "value": value}))
            .await
            .expect("secret_set");
    }
    let spec = json!({
        "method": "GET",
        "url": "https://api.example.com/v1/ping",
        "headers": {"Authorization": "Bearer {{secret.spkind_missing}}"},
        "args_schema": {"type": "object"},
    });
    let err = client
        .tools_call("host.tool.publish", json!({"name": "spkind_ac06", "kind": "http", "spec": spec}))
        .await
        .expect_err("a spec referencing an unset secret is refused");
    assert_eq!(err.error_code.as_deref(), Some("invalid_spec"), "{err:?}");
    assert_eq!(err.data["field"], "headers");
    assert_eq!(
        err.data["fix"],
        json!({"tool": "host.secret.set", "args": {"name": "spkind_missing"}})
    );
    assert_eq!(err.data["known_secret_names"], json!(["a", "b"]));
    let envelope = format!("{} {}", err.message, err.data);
    for value in ["spkind-value-of-a", "spkind-value-of-b"] {
        assert!(!envelope.contains(value), "secret value leaked into the envelope: {envelope}");
    }
}
