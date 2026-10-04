//! PRD-mcphost-implicit-signup
//! AC7 (P0) — Given the synthorg journey and tool-surface conformance
//! harnesses at the landing commit, When run against a build of this PRD,
//! Then no assertion expects `tenant_key_missing` on `/mcp` and both runs
//! are green on the fixture host.
//!
//! The synthorg journey harness and the tool-surface conformance harness
//! named by this AC live in `~/repos/synthorg`, outside this repo/worktree
//! and outside its own `cargo test` gate -- this test instead pins the
//! exact behavioral contract those harnesses depend on, in-repo: a
//! representative sweep of `host.*`/`billing.*` tool names, each called
//! completely bare (fresh session, no header, no `tenant_key` argument)
//! on `/mcp`, never returns `tenant_key_missing` -- requirement 4's own
//! wording, "no longer reachable for host.*/billing.* on /mcp". A generous
//! rate-limit override keeps the sweep itself from tripping AC3's own
//! limiter.
//!
//! The two named external harnesses were in fact located, fixed to drop
//! their own now-obsolete `tenant_key_missing`-on-`no_key` expectation,
//! and run green (synthorg commit `7b610cb`, hand-run outside this
//! worktree's own gate) -- see
//! `docs/receipts/implsign-ac07-synthorg-cross-repo.md` and
//! `agent/test-map.json`'s `AC7` entry for the full record.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn bare_calls_across_the_host_and_billing_surface_never_return_tenant_key_missing() {
    let server = TestServer::start_with_signup_rate_limit(1000).await;

    let names = [
        "host.whoami",
        "host.tool_publish",
        "host.tool_list",
        "host.tool_remove",
        "host.usage",
        "host.changelog",
        "host.export",
        "host.tool_call",
        "host.tool_share",
        "host.tool_test",
        "host.secret_list",
        "host.catalog.search",
        "host.group.list",
        "billing.checkout",
        "billing.status",
        // Exempt names (Non-goals / requirement 1): never implicitly sign
        // up, but must equally never return tenant_key_missing -- they
        // never have, and this PRD must not change that.
        "host.quickstart",
        "billing.plans",
        "host.redeem",
        "signup",
    ];

    for name in names {
        // A fresh, independent session per call -- each one genuinely
        // bare, never riding a prior call's binding.
        let client = McpClient::new(&server.base_url);
        let result = client.tools_call(name, json!({})).await;
        if let Err(err) = result {
            assert_ne!(
                err.error_code.as_deref(),
                Some("tenant_key_missing"),
                "a bare call to {name} must never return tenant_key_missing: {err:?}"
            );
        }
    }
}
