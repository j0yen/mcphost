//! PRD-mcphost-tool-naming-convention-and-aliases
//! AC5 (P0) -- Given the alias metrics, When 3 alias calls and 2 canonical
//! calls happen for `host.tool.share`, Then the counter named in
//! `docs/metrics.md` (`/healthz`'s `tool_aliases` field) reads alias=3
//! canonical=2.
//!
//! Uses `host.tool.list`/`host.tool_list` rather than `host.tool.share`
//! itself (no published/shared tool fixture needed) -- the metric is
//! recorded identically for every tracked canonical.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn healthz_tool_aliases_counts_alias_and_canonical_calls_separately() {
    let server = TestServer::start().await;
    let (_tenant, key) = common::signup(&server.base_url, "ac05-caller").await;
    let authed = McpClient::with_bearer(&server.base_url, &key);

    for _ in 0..3 {
        authed.tools_call("host.tool_list", json!({})).await.expect("alias call");
    }
    for _ in 0..2 {
        authed.tools_call("host.tool.list", json!({})).await.expect("canonical call");
    }

    // The full diagnostic body (including tool_aliases) is admin-only --
    // an anonymous GET gets only {"ok": true} (src/http.rs's healthz_response).
    let healthz: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/healthz", server.base_url))
        .bearer_auth(common::ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse /healthz JSON");

    let counts = &healthz["tool_aliases"]["host.tool.list"];
    assert_eq!(counts["alias"], 3, "healthz: {healthz}");
    assert_eq!(counts["canonical"], 2, "healthz: {healthz}");

    // requirement 5: present at zero for a canonical nobody has called
    // this way yet -- an operator can see which aliases have gone quiet,
    // not just the ones with traffic.
    assert_eq!(healthz["tool_aliases"]["host.key.rotate"]["alias"], 0, "healthz: {healthz}");
}
