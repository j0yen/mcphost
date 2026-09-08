//! AC1 -- Given a spec with `upstream` (http shape) published with
//! `kind: http`, When publish runs, Then the tool is http-kind and the
//! publish response says so.

mod common;
use common::{TempDataDir, TestServer, http_kind_registry, signup};
use serde_json::json;

#[tokio::test]
async fn upstream_spec_with_kind_http_publishes_as_http() {
    let _envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Kind Honor AC1").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "latency_probe",
                "kind": "http",
                "spec": {
                    "upstream": {
                        "url": "https://api.example.com/items/{id}",
                        "method": "GET",
                        "params": {"id": {"in": "path"}},
                    },
                },
            }),
        )
        .await
        .expect("upstream http spec requested as http publishes");

    let structured = common::extract_structured(&result);
    assert_eq!(structured["kind"], json!("http"));
}
