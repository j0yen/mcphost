//! AC4 (P0) -- PRD-mcphost-server-capabilities-from-registry. `GET
//! /capabilities.json` with no key, 100 times in 10 s: every response is 200
//! with `primitives.tools.implemented == true` and `version` equal to
//! `CARGO_PKG_VERSION`, and no signup or tenant row is created.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer};
use serde_json::{Value, json};

async fn tenant_count(server: &TestServer) -> usize {
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let out = admin.tools_call("admin.tenants", json!({})).await.expect("admin.tenants");
    common::extract_structured(&out)["tenants"].as_array().map_or(0, Vec::len)
}

#[tokio::test]
async fn capabilities_json_is_keyless_fast_and_creates_no_rows() {
    let server = TestServer::start().await;
    let before = tenant_count(&server).await;
    let signups_before = server.state.db.probe_tenant_count().await.expect("probe");
    let http = reqwest::Client::new();
    let url = format!("{}/capabilities.json", server.base_url);

    crate::deadline_budget!(10_000, {
    for i in 0..100 {
        let resp = http.get(&url).send().await.expect("get");
        assert_eq!(resp.status(), 200, "fetch {i}");
        assert_eq!(
            resp.headers().get("cache-control").and_then(|v| v.to_str().ok()),
            Some("max-age=60")
        );
        let doc: Value = resp.json().await.expect("json");
        assert_eq!(doc["primitives"]["tools"]["implemented"], true, "{doc}");
        assert_eq!(doc["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(doc["primitives"]["prompts"]["implemented"], false);
    }
    });

    let filtered: Value = http.get(format!("{url}?implemented=1")).send().await.expect("get").json().await.expect("json");
    let mut names: Vec<&str> = filtered["primitives"].as_object().expect("object").keys().map(String::as_str).collect();
    names.sort_unstable();
    let mut implemented = mcphost::capabilities::implemented_names();
    implemented.sort_unstable();
    assert_eq!(names, implemented, "?implemented=1 keeps only implemented rows");

    assert_eq!(tenant_count(&server).await, before, "no tenant row may be created");
    assert_eq!(server.state.db.probe_tenant_count().await.expect("probe"), signups_before);
}
