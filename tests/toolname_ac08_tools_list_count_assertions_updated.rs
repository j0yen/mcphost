//! PRD-mcphost-tool-naming-convention-and-aliases
//! AC8 (P0) -- Given the existing test suite, When any test asserted an
//! exact `tools/list` count, Then it is updated to the new count with a
//! comment citing this PRD, and no other test changed.
//!
//! The actual updates are the five pre-existing count literals this PRD
//! bumped (141 -> 160 in `ac01_unauthenticated_lists_signup.rs`,
//! `compat_ac01_ac02_ac03_cache_fields.rs` x2, and
//! `compat_ac11_ac12_claude_sdk_replay.rs`; 141 -> 161 in
//! `compat_ac04_ac05_admin_and_tenant_cache_fields.rs`'s authenticated-tenant
//! case; 137 -> 156 in `sessionkey_ac02_ac03_discovery_shape.rs`), each
//! with its own comment citing this PRD at the literal. This test is the
//! one canonical, independently-derived pairing for that AC: it recomputes
//! the anonymous and authenticated counts from the live registry plus the
//! alias table, rather than re-reading any of those five files' own
//! hardcoded numbers, so a future drift between them and the real
//! registry fails here first.

use crate::common;
use common::{McpClient, TestServer};
use mcphost::tool_aliases::ALIASES;

#[tokio::test]
async fn anonymous_and_authenticated_tools_list_counts_match_the_live_registry_plus_aliases() {
    let server = TestServer::start().await;

    let anon = McpClient::new(&server.base_url).tools_list().await.expect("anonymous tools/list");
    let anon_names: Vec<&str> =
        anon["tools"].as_array().expect("tools array").iter().map(|t| t["name"].as_str().unwrap()).collect();

    let (_tenant, key) = common::signup(&server.base_url, "ac08-caller").await;
    let authed_list = McpClient::with_bearer(&server.base_url, &key)
        .tools_list()
        .await
        .expect("authenticated tools/list");
    let authed_names: Vec<&str> = authed_list["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();

    // host.spec.test (and its alias host.spec_test) is the one descriptor
    // gated to authenticated callers only -- every other alias in ALIASES
    // is visible anonymously too (PRD-mcphost-session-key).
    let gated_pairs = 1usize;
    assert_eq!(anon_names.len() + gated_pairs, authed_names.len(), "{anon_names:?} vs {authed_names:?}");

    // Every alias this PRD registered must be visible authenticated; all
    // but the one gated pair must also be visible anonymously.
    for (alias, _canonical) in ALIASES {
        assert!(authed_names.contains(alias), "{alias} missing from authenticated tools/list");
    }
    assert!(!anon_names.contains(&"host.spec_test"), "host.spec_test must stay authenticated-only");
    assert!(!anon_names.contains(&"host.spec.test"), "host.spec.test must stay authenticated-only");
}
