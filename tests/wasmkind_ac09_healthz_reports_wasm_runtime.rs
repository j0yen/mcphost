//! PRD-mcphost-wasm-kind
//! AC9 -- Given the admin `/healthz`, When read, Then it reports the wasm
//! runtime's availability and version.

use crate::common;
use common::{ADMIN_KEY, wasm_kind_registry};

#[tokio::test]
async fn healthz_reports_wasm_runtime_version_when_the_kind_is_registered() {
    let server = common::TestServer::start_with_kinds(wasm_kind_registry()).await;

    let http = reqwest::Client::new();
    let healthz = http
        .get(format!("{}/healthz", server.base_url))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz");
    assert_eq!(healthz.status(), 200);
    let body: serde_json::Value = healthz.json().await.expect("json body");
    assert_eq!(
        body["wasm_runtime_version"],
        serde_json::json!(mcphost::kinds::wasm::WASM_RUNTIME_VERSION),
        "/healthz must report the wasm runtime's version when the kind is registered: {body}"
    );
}

#[tokio::test]
async fn healthz_reports_no_wasm_runtime_when_the_kind_is_not_registered() {
    let server = common::TestServer::start().await;

    let http = reqwest::Client::new();
    let healthz = http
        .get(format!("{}/healthz", server.base_url))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz");
    let body: serde_json::Value = healthz.json().await.expect("json body");
    assert_eq!(
        body["wasm_runtime_version"],
        serde_json::Value::Null,
        "a server without the wasm kind registered must not claim a wasm runtime is available: {body}"
    );
}
