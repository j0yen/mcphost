//! PRD-mcphost-invalid-spec-error-carries-kind-fields
//! AC1 (P0) — Given an http spec `{"url":"https://example.com"}`, When
//! `host.tool.publish` validates it, Then the refusal is `invalid_spec` with
//! `data.kind "http"`, `data.field "method"`, a `fields` row for `method`
//! with required ``unless `upstream` ``, and `minimal_specs` containing
//! `{"method":…,"url":…}`.

use crate::common::{McpClient, TestServer, http_kind_registry, signup};
use serde_json::json;

#[tokio::test]
async fn spkind_ac01_http_missing_method() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Spkind01").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let err = client
        .tools_call(
            "host.tool.publish",
            json!({"name": "spkind_ac01", "kind": "http", "spec": {"url": "https://example.com"}}),
        )
        .await
        .expect_err("an http spec without method is refused");
    assert_eq!(err.error_code.as_deref(), Some("invalid_spec"), "{err:?}");
    let data = &err.data;
    assert_eq!(data["kind"], "http");
    assert_eq!(data["field"], "method");
    let row = data["fields"]
        .as_array()
        .expect("fields is an array")
        .iter()
        .find(|r| r["name"] == "method")
        .expect("a fields row for method");
    assert_eq!(row["required"], "unless `upstream`");
    assert!(row["ty"].is_string() && row["meaning"].is_string(), "{row}");
    let specs = data["minimal_specs"].as_array().expect("minimal_specs is an array");
    assert!(
        specs.iter().any(|s| s.get("method").is_some() && s.get("url").is_some()),
        "a minimal spec carries method and url: {specs:?}"
    );
}
