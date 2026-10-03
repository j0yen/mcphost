//! PRD-mcphost-tool-naming-convention-and-aliases
//! AC2 — Given a client calling `host.tool_share` with valid arguments,
//! When dispatched, Then the result equals a call to `host.tool.share`
//! with the same arguments, the response carries the deprecation hint
//! (where the protocol allows), and exactly one `tool_deprecated_alias`
//! log line appears for that tenant that day across 5 calls.
//!
//! The dispatch-equivalence and `_meta.deprecated` hint assertions drive a
//! real server over real HTTP. The "exactly one log line per tenant per
//! day" half instead exercises `tool_aliases::mark_alias_logged` directly
//! -- the exact dedup primitive `handler::call_tool`'s success path calls
//! before every `tracing::info!("tool_deprecated_alias")` -- rather than
//! capturing real tracing output: this crate's test suite is
//! consolidated into ~10 binaries of ~1000 tests each
//! (PRD-mcphost-test-suite-consolidation), and `tracing`'s per-callsite
//! `Interest` cache is a single process-wide static that any concurrently
//! running, unrelated test calling an alias tool can race (see
//! `tests/sessbind_ac10_request_log_records_the_upgraded_status.rs`'s own
//! doc comment for the mechanics) -- a capture-based assertion here would
//! be flaky under the full parallel suite even though the underlying
//! dedup logic is correct and deterministic.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::Value;

#[tokio::test]
async fn alias_call_matches_canonical_and_carries_the_deprecation_hint() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC2 Owner").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            serde_json::json!({"name": "tool_t", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish tool_t");

    let share_args = serde_json::json!({"name": "tool_t", "visibility": "public"});

    let canonical_result = client
        .tools_call("host.tool.share", share_args.clone())
        .await
        .expect("canonical call");
    let canonical_structured = extract_structured(&canonical_result);
    assert!(
        canonical_result.get("_meta").and_then(|m| m.get("deprecated")).is_none(),
        "a canonical call must carry no _meta.deprecated hint: {canonical_result}"
    );

    let mut last_alias_result = Value::Null;
    for _ in 0..5 {
        last_alias_result = client
            .tools_call("host.tool_share", share_args.clone())
            .await
            .expect("alias call");
    }

    let alias_structured = extract_structured(&last_alias_result);
    assert_eq!(
        alias_structured, canonical_structured,
        "an alias call's result must equal the canonical call's result for the same arguments"
    );

    let deprecated = last_alias_result
        .get("_meta")
        .and_then(|m| m.get("deprecated"))
        .unwrap_or_else(|| panic!("alias call result must carry _meta.deprecated: {last_alias_result}"));
    assert_eq!(deprecated["replaced_by"], "host.tool.share", "{deprecated}");
    assert_eq!(deprecated["sunset"], "2026-12-31", "{deprecated}");
}

/// The "at most once per tenant per day" half -- see this file's own
/// header doc for why it drives the dedup primitive directly rather than
/// capturing tracing output.
#[test]
fn log_dedup_fires_once_per_tenant_per_day_across_five_calls() {
    let tenant_id = 424_242;
    let day = mcphost::state::date_from_unix(mcphost::state::now_unix());

    let fired: Vec<bool> =
        (0..5).map(|_| mcphost::tool_aliases::mark_alias_logged(tenant_id, "host.tool_share", &day)).collect();

    assert_eq!(
        fired,
        vec![true, false, false, false, false],
        "only the first of 5 same-day calls for this tenant/alias should fire the log line"
    );

    // A different tenant, or a different alias, or a different day, gets
    // its own independent slot.
    assert!(mcphost::tool_aliases::mark_alias_logged(tenant_id + 1, "host.tool_share", &day));
    assert!(mcphost::tool_aliases::mark_alias_logged(tenant_id, "host.secret_set", &day));
    assert!(mcphost::tool_aliases::mark_alias_logged(tenant_id, "host.tool_share", "1999-01-01"));
}
