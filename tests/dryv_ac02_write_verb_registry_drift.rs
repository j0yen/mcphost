//! PRD-mcphost-dry-run-on-write-verbs
//! AC2 -- Given `host_tools(kinds, true)` plus aliases, When the drift test
//! runs, Then every verb matching the R2 pattern is in the `WriteVerb`
//! registry and every registry entry dispatches; a test that adds a fake
//! `host.docs.archive` to the tool list fails with its name in the message.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::write_verbs::{DRY_RUN_VERBS, DryRun, NO_DRY_RUN_REASONS, NO_DRY_RUN_VERBS, WRITE_VERBS, lookup};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// R2's pattern, applied to the `.`/`_`-separated words of a name, plus
/// `archive`: the AC's own fake verb must trip the check, and the PRD's
/// pattern as written would not have matched it.
const WRITE_WORDS: &[&str] = &[
    "archive",
    "delete", "drop", "purge", "revoke", "remove", "offboard", "rotate", "set", "put", "create", "append", "insert",
    "publish", "send", "post", "fire", "pause", "resume", "promote",
];

fn matches_write_pattern(name: &str) -> bool {
    (name.starts_with("host.") || name.starts_with("host_") || name.starts_with("billing"))
        && name.split(['.', '_']).any(|w| WRITE_WORDS.contains(&w))
}

/// The names (canonical, aliases and flattened forms) `tools/list` advertises.
async fn advertised_names(client: &McpClient) -> Vec<String> {
    let listed = client.tools_list().await.expect("tools/list");
    listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().expect("tool name").to_string())
        .collect()
}

/// The drift check itself, over any tool-name list: names matching the R2
/// pattern that the registry does not know. Flattened (`host_tool_remove`)
/// names are skipped -- they are clones of a dotted canonical in the list.
fn unregistered(names: &[String]) -> Vec<String> {
    names
        .iter()
        .filter(|n| matches_write_pattern(n))
        .filter(|n| lookup(n).is_none())
        .filter(|n| !is_flattened_clone(n, names))
        .cloned()
        .collect()
}

fn is_flattened_clone(name: &str, names: &[String]) -> bool {
    name.starts_with("host_")
        && names
            .iter()
            .any(|c| c.contains('.') && mcphost::tool_aliases::flattened_form(c).as_deref() == Some(name))
}

#[tokio::test]
async fn every_write_pattern_verb_in_tools_list_is_registered_and_every_entry_dispatches() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Dryv AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let names = advertised_names(&client).await;

    let missing = unregistered(&names);
    assert!(
        missing.is_empty(),
        "write verb(s) {missing:?} are neither in DRY_RUN_VERBS nor NO_DRY_RUN_VERBS with a reason -- add them to \
         WRITE_VERBS in src/write_verbs.rs"
    );

    // Derived lists cannot drift from the table, and every opt-out has a
    // reason from the closed set.
    assert_eq!(DRY_RUN_VERBS.len() + NO_DRY_RUN_VERBS.len(), WRITE_VERBS.len());
    for (verb, reason) in NO_DRY_RUN_VERBS {
        assert!(NO_DRY_RUN_REASONS.contains(&reason), "{verb}: unknown reason {reason}");
    }
    let unique: BTreeSet<&str> = WRITE_VERBS.iter().map(|v| v.name).collect();
    assert_eq!(unique.len(), WRITE_VERBS.len(), "duplicate registry rows");

    // Every entry dispatches: it is advertised, and a `dry_run: true` call
    // (rolled back for Supported, refused for NoDryRun -- neither commits)
    // never answers "unknown tool".
    for verb in WRITE_VERBS {
        assert!(names.iter().any(|n| n == verb.name), "{} is in the registry but not advertised", verb.name);
        let outcome = client.tools_call(verb.name, json!({"dry_run": true})).await;
        if let Err(e) = outcome {
            let code = e.error_code.clone().unwrap_or_default();
            assert!(
                code != "unknown_tool" && code != "tool_not_found",
                "{} is in the registry but does not dispatch: {e:?}",
                verb.name
            );
            if let DryRun::NoDryRun(reason) = verb.dry_run {
                assert_eq!(code, "args_invalid", "{}: {e:?}", verb.name);
                assert_eq!(e.data["no_dry_run_reason"], Value::String(reason.to_string()), "{}", verb.name);
            }
        }
    }
}

#[tokio::test]
async fn a_fake_docs_archive_verb_fails_the_drift_check_by_name() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Dryv AC2 Fake Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let mut names = advertised_names(&client).await;
    assert!(unregistered(&names).is_empty(), "baseline list must be clean");

    names.push("host.docs.archive".to_string());
    let missing = unregistered(&names);
    assert_eq!(missing, vec!["host.docs.archive".to_string()]);
    let message = format!("write verb(s) {missing:?} are neither in DRY_RUN_VERBS nor NO_DRY_RUN_VERBS");
    assert!(message.contains("host.docs.archive"), "{message}");
}
