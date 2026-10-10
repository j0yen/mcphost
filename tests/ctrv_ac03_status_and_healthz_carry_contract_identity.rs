//! PRD-mcphost-contract-version-reported AC3 — Given `GET /status.json` and
//! `GET /healthz` with the admin bearer, When parsed, Then both carry
//! `contract_version` and `contract_sha` equal to whoami's; Given anonymous
//! `GET /healthz`, Then the body is exactly `{"ok":true}`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use serde_json::{Value, json};

#[tokio::test]
async fn status_and_admin_healthz_carry_whoami_contract_identity_and_anon_healthz_is_minimal() {
    let server = TestServer::start().await;
    let whoami = extract_structured(
        &McpClient::with_bearer(&server.base_url, ADMIN_KEY)
            .tools_call("host.whoami", json!({}))
            .await
            .expect("admin whoami"),
    );
    let http = reqwest::Client::new();

    let status: Value = http
        .get(format!("{}/status.json", server.base_url))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("status.json")
        .json()
        .await
        .expect("status.json is json");
    let healthz: Value = http
        .get(format!("{}/healthz", server.base_url))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("healthz")
        .json()
        .await
        .expect("healthz is json");

    for (label, body) in [("/status.json", &status), ("/healthz", &healthz)] {
        assert_eq!(body["contract_version"], whoami["contract_version"], "{label}: {body}");
        assert_eq!(body["contract_sha"], whoami["contract_sha"], "{label}: {body}");
        assert!(body["contract_sha"].as_str().is_some_and(|s| s.len() == 64), "{label}");
    }

    let anon = http
        .get(format!("{}/healthz", server.base_url))
        .send()
        .await
        .expect("anon healthz")
        .text()
        .await
        .expect("body");
    let anon: serde_json::Value = serde_json::from_str(&anon).expect("anon healthz json");
    let mut keys: Vec<&str> = anon.as_object().expect("object").keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["git_sha", "ok", "version"], "anonymous /healthz keys: {anon}");
    assert_eq!(anon["ok"], true, "anonymous /healthz body: {anon}");
    assert!(anon.get("contract_version").is_none(), "contract_version leaked: {anon}");
    assert!(anon.get("contract_sha").is_none(), "contract_sha leaked: {anon}");
}
