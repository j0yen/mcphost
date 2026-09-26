//! PRD-mcphost-runs-end-user-subject
//! AC5 (P0) — Given the existing runs fixture with no end user, When
//! `host.runs.list` output is compared before and after this PRD, Then
//! every row is identical except the added `end_user: null` key.
//!
//! The "before" shape is the exact key set `run_to_json` produced prior to
//! this PRD (frozen here, since the pre-PRD code no longer exists to run
//! side by side) -- a regression on either an added/removed/renamed key
//! (other than `end_user`) or a null-vs-present `end_user` fails this test.
//! Rebased onto PR #46 (run-result-overflow-to-state), which added
//! `counters`/`error`/`result_ref` to every run row ahead of this PRD --
//! all three are part of the frozen "before" set below, same as every
//! other pre-existing key.

use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[tokio::test]
async fn list_output_is_unchanged_but_for_the_added_end_user_key() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC5 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echoer", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish ok");
    client
        .tools_call(&format!("{ns}.echoer"), json!({"hello": "world"}))
        .await
        .expect("call ok");

    let runs = extract_structured(
        &client
            .tools_call("host.runs.list", json!({}))
            .await
            .expect("runs.list ok"),
    );
    let list = runs["runs"].as_array().expect("runs array");
    assert_eq!(list.len(), 1, "exactly one runs row: {runs}");
    let run = list[0].as_object().expect("run is an object");

    let pre_prd_keys: BTreeSet<&str> = [
        "run_id",
        "tool",
        "trigger",
        "trigger_ref",
        "status",
        "progress",
        "counters",
        "result",
        "result_ref",
        "purged",
        "error_class",
        "error",
        "started_unix",
        "finished_unix",
        "duration_ms",
        "deadline_s",
        "attempt",
        "manual",
        "test",
    ]
    .into_iter()
    .collect();
    let actual_keys: BTreeSet<&str> = run.keys().map(String::as_str).collect();
    let expected_keys: BTreeSet<&str> =
        pre_prd_keys.union(&["end_user"].into_iter().collect()).copied().collect();
    assert_eq!(
        actual_keys, expected_keys,
        "run row's key set must be exactly the pre-PRD set plus end_user: {run:?}"
    );
    assert_eq!(run["end_user"], Value::Null, "no end user on this call: {run:?}");
}
