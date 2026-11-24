//! PRD-mcphost-invalid-spec-message-carries-fix
//! AC1 (P0) — Given an http spec `{"url":"https://example.com"}`, When
//! `host.tool.publish` refuses it, Then `message` is `invalid spec: spec: missing
//! field `method`; field `method` is required unless `upstream` is set; minimal
//! spec: {"method":"GET","url":"https://api.example.com/v1/things"}; docs:
//! https://mcphost.dev/docs/kinds/http`, its `docs:` clause equals
//! `data.docs_url`, `error_code` is `invalid_spec`, and `data` passes spkind_ac01.

use crate::common::{McpClient, TestServer, http_kind_registry, signup};
use serde_json::json;

#[tokio::test]
async fn spmsg_ac01() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Spmsg01").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let err = client
        .tools_call(
            "host.tool.publish",
            json!({"name": "spmsg_ac01", "kind": "http", "spec": {"url": "https://example.com"}}),
        )
        .await
        .expect_err("an http spec without method is refused");
    assert_eq!(err.error_code.as_deref(), Some("invalid_spec"), "{err:?}");
    assert_eq!(
        err.message,
        "invalid spec: spec: missing field `method`; field `method` is required unless `upstream` is set; \
         minimal spec: {\"method\":\"GET\",\"url\":\"https://api.example.com/v1/things\"}; \
         docs: https://mcphost.dev/docs/kinds/http"
    );
    let docs = err.message.rsplit_once("; docs: ").expect("a docs clause").1;
    assert_eq!(Some(docs), err.data["docs_url"].as_str());
    // `data` is unchanged: the same assertions spkind_ac01 makes.
    assert_eq!(err.data["kind"], "http");
    assert_eq!(err.data["field"], "method");
    let row = err.data["fields"]
        .as_array()
        .expect("fields is an array")
        .iter()
        .find(|r| r["name"] == "method")
        .expect("a fields row for method");
    assert_eq!(row["required"], "unless `upstream`");
    let specs = err.data["minimal_specs"].as_array().expect("minimal_specs is an array");
    assert!(specs.iter().any(|s| s.get("method").is_some() && s.get("url").is_some()), "{specs:?}");
}
