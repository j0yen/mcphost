//! PRD-mcphost-schema-bound-agreement
//! AC3 (P0) -- Given `SCHEMA_VALID_REFUSALS`, When its test runs, Then every
//! entry names a tool, a JSON pointer, a reason and a `tests/` file whose test
//! asserts the handler refusal still fires for that pointer, and the entry
//! count equals a snapshot constant pinned in the same file, so adding an entry
//! needs a visible bump.

use crate::common;
use crate::vbd_calls;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use mcphost::error_registry::lookup;
use mcphost::host_args::{SCHEMA_VALID_REFUSALS, SCHEMA_VALID_REFUSALS_SNAPSHOT};
use mcphost::precheck::precheck_args as validate_host_args;
use mcphost::kinds::KindRegistry;
use serde_json::{Value, json};
use std::time::Duration;

/// This file: every entry's `test` names it.
const THIS_FILE: &str = "sba_ac03_schema_valid_refusals_still_fire.rs";

/// Schema-valid calls the boundary generator cannot derive (an upper bound the
/// schema does not state), one per entry that needs one.
fn explicit_probe(tool: &str, pointer: &str) -> Option<Value> {
    let multibyte = |bytes: usize| "\u{e9}".repeat(bytes / 2 + 1);
    Some(match (tool, pointer) {
        ("host.agent.profile_set", "/description") => json!({"description": multibyte(mcphost::agents::MAX_DESCRIPTION_BYTES)}),
        ("host.agent.profile_set", "/tags") => json!({"tags": [multibyte(mcphost::agents::MAX_TAG_BYTES)]}),
        ("host.agent.contact_request", "/note") => json!({"address": "@sba_ac03", "note": multibyte(512)}),
        ("host.email.send", "/subject") => json!({"to": "a@example.com", "subject": multibyte(mcphost::email_send::SUBJECT_MAX_BYTES), "text": "x"}),
        ("host.email.send", "/text") => json!({"to": "a@example.com", "subject": "x", "text": "x".repeat(mcphost::email_send::TEXT_MAX_BYTES + 1)}),
        ("host.notify.send", "/text") => json!({"level": "info", "text": "x".repeat(mcphost::notify::TEXT_MAX_BYTES + 1)}),
        _ => return None,
    })
}

async fn refusal(client: &McpClient, tool: &str, args: &Value) -> Option<(String, String)> {
    match tokio::time::timeout(Duration::from_secs(20), client.tools_call(tool, args.clone())).await {
        Ok(Err(e)) => Some((e.error_code.unwrap_or_default(), e.message)),
        _ => None,
    }
}

#[tokio::test]
async fn sba_ac03_every_entry_names_what_it_pins_and_the_refusal_still_fires() {
    let kinds = KindRegistry::with_builtin();
    let server = TestServer::start_with_signup_rate_limit(100_000).await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let descriptors = vbd_calls::all_descriptors();
    let mut dead = Vec::new();
    for entry in SCHEMA_VALID_REFUSALS {
        assert!(!entry.tool.is_empty() && entry.pointer.starts_with('/') && !entry.reason.is_empty(), "malformed entry {entry:?}");
        assert_eq!(entry.test, THIS_FILE, "{} {} names a different test file", entry.tool, entry.pointer);
        assert!(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join(entry.test).is_file(), "{} is not under tests/", entry.test);
        if entry.reason == "byte-bound" {
            assert!(!entry.konst.is_empty(), "{} {}: a byte-bound entry names its const", entry.tool, entry.pointer);
        }
        let tool = descriptors.iter().find(|t| t.name == entry.tool).unwrap_or_else(|| panic!("{} is not a descriptor", entry.tool));
        let property = entry.pointer.trim_start_matches('/');
        let candidates: Vec<Value> = match explicit_probe(entry.tool, entry.pointer) {
            Some(args) => vec![args],
            None => vbd_calls::boundary_calls(&tool.input_schema).into_iter().filter(|(p, _, _)| p == entry.pointer).map(|(_, _, a)| a).collect(),
        };
        let mut fired = false;
        for args in candidates {
            if validate_host_args(&kinds, entry.tool, &args).is_err() {
                continue;
            }
            let fixture;
            let client: &McpClient = if vbd_calls::is_admin(tool) {
                &admin
            } else {
                let (_, key) = signup(&server.base_url, "SBA AC3").await;
                fixture = McpClient::with_bearer(&server.base_url, &key);
                &fixture
            };
            let Some((code, message)) = refusal(client, entry.tool, &args).await else { continue };
            let family = lookup(&code).map(|r| r.family.as_str()).unwrap_or_default();
            if matches!(family, "args" | "spec") && message.contains(property) {
                fired = true;
                break;
            }
        }
        if !fired {
            dead.push(format!("{} {}", entry.tool, entry.pointer));
        }
    }
    assert!(dead.is_empty(), "entries whose handler refusal no longer fires (remove them from SCHEMA_VALID_REFUSALS):\n{}", dead.join("\n"));
}

#[test]
fn sba_ac03_entry_count_equals_the_pinned_snapshot() {
    assert_eq!(SCHEMA_VALID_REFUSALS.len(), SCHEMA_VALID_REFUSALS_SNAPSHOT, "bump SCHEMA_VALID_REFUSALS_SNAPSHOT with the entry (review refuses growth)");
    let mut keys: Vec<_> = SCHEMA_VALID_REFUSALS.iter().map(|r| (r.tool, r.pointer)).collect();
    keys.sort();
    let n = keys.len();
    keys.dedup();
    assert_eq!(keys.len(), n, "a (tool, pointer) pair appears twice");
}
