//! PRD-mcphost-tool-naming-convention-and-aliases
//! AC5 — Given the alias metrics, When 3 alias calls and 2 canonical
//! calls happen for a tool with a deprecated alias, Then the counter
//! named in `docs/metrics.md` (`tool_alias_calls{tool}`, read off admin
//! `/healthz`) reads alias=3 canonical=2.
//!
//! Uses `host.secret_list`/`host.secret.list` rather than the PRD's own
//! `host.tool_share` example: `tool_alias_calls` (`tool_aliases::
//! record_call`/`alias_metrics_snapshot`) is a process-wide, in-memory
//! counter, not reset or scoped per test server (same tradeoff
//! `help_url_served{code}` already makes, see `docs/metrics.md`), and
//! this crate's ~1000-test suite binaries call `host.tool_share`/
//! `host.tool_publish` constantly elsewhere, concurrently, which would
//! make an exact-count assertion on THAT pair flaky under the full
//! parallel suite. `host.secret_list` is called from only a handful of
//! other test files, is a no-argument, always-succeeds read (no
//! publish/share setup needed), and exercises the exact same
//! `record_call` hook on the exact same counter mechanism this AC is
//! about -- so a before/after delta on it proves the same thing with a
//! vanishingly small collision window instead of a wide one.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};

async fn healthz_tool_alias_calls(base_url: &str, canonical: &str) -> (u64, u64) {
    let body: serde_json::Value = reqwest::Client::new()
        .get(format!("{base_url}/healthz"))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse /healthz");
    let entry = &body["tool_alias_calls"][canonical];
    (entry["alias"].as_u64().unwrap_or(0), entry["canonical"].as_u64().unwrap_or(0))
}

#[tokio::test]
async fn three_alias_and_two_canonical_calls_move_the_counter_by_exactly_that_delta() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let (alias_before, canonical_before) = healthz_tool_alias_calls(&server.base_url, "host.secret.list").await;

    for _ in 0..3 {
        client.tools_call("host.secret_list", serde_json::json!({})).await.expect("alias call");
    }
    for _ in 0..2 {
        client.tools_call("host.secret.list", serde_json::json!({})).await.expect("canonical call");
    }

    let (alias_after, canonical_after) = healthz_tool_alias_calls(&server.base_url, "host.secret.list").await;

    assert_eq!(alias_after - alias_before, 3, "3 alias calls must move the alias counter by exactly 3");
    assert_eq!(
        canonical_after - canonical_before,
        2,
        "2 canonical calls must move the canonical counter by exactly 2"
    );
}
