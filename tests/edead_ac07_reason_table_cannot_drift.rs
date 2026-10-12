//! PRD-mcphost-event-trigger-from-webhook AC7 (P1) — Given
//! `DEAD_LETTER_REASONS`, When a test enumerates every `AppError` code
//! constructed in the hook refusal path, Then each is in the table and the
//! `host.trigger.dead_letters` description lists every code.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::deadletters::{DEAD_LETTER_REASONS, reason_for_error};
use mcphost::errors::AppError;
use std::collections::BTreeSet;

/// The text of `fn <name>` up to the next top-level item.
fn fn_body<'a>(src: &'a str, signature: &str) -> &'a str {
    let start = src.find(signature).unwrap_or_else(|| panic!("{signature} not found in src/hooks.rs"));
    let rest = &src[start..];
    let end = rest[signature.len()..]
        .find("\n}\n")
        .map(|i| i + signature.len())
        .unwrap_or(rest.len());
    &rest[..end]
}

/// Every `code: "<literal>"` in `text`.
fn code_literals(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = text;
    while let Some(i) = rest.find("code: \"") {
        // `reason_code:` / `error_code:` are letter fields, not AppError codes.
        let is_field = rest[..i].ends_with(|c: char| c.is_alphanumeric() || c == '_');
        rest = &rest[i + "code: \"".len()..];
        if let (false, Some(j)) = (is_field, rest.find('"')) {
            out.insert(rest[..j].to_string());
        }
    }
    out
}

/// The `AppError` codes the hook refusal path constructs, read off the
/// source of the functions `handle_hook` refuses through.
fn constructed_codes() -> BTreeSet<String> {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/hooks.rs")).expect("read hooks.rs");
    let handle_hook = fn_body(&src, "async fn handle_hook(");
    let mut codes = BTreeSet::new();
    for sig in [
        "fn hook_not_found(",
        "fn signature_invalid(",
        "fn signature_invalid_reason(",
        "fn verify_stripe_style(",
        "fn verify_event(",
    ] {
        codes.extend(code_literals(fn_body(&src, sig)));
    }
    codes.extend(code_literals(handle_hook));
    // Constructed by variant rather than a `code:` literal.
    if fn_body(&src, "async fn resolve_secret(").contains("AppError::SecretMissing") {
        codes.insert(AppError::SecretMissing(String::new()).code().to_string());
    }
    if handle_hook.contains("bans::enforce") {
        codes.insert("banned".to_string());
    }
    codes
}

#[test]
fn every_refusal_code_is_in_the_reason_table() {
    let codes = constructed_codes();
    for expected in [
        "hook_not_found",
        "signature_invalid",
        "event_body_too_large",
        "events_rate_limited",
        "secret_missing",
        "banned",
    ] {
        assert!(codes.contains(expected), "scan missed {expected}: {codes:?}");
    }
    for code in &codes {
        assert!(
            DEAD_LETTER_REASONS.iter().any(|r| r.error_code == code),
            "hook refusal code '{code}' is not in DEAD_LETTER_REASONS"
        );
    }
    // Every stored reason resolves from its own error code.
    for r in DEAD_LETTER_REASONS.iter().filter(|r| r.stored) {
        if !r.error_code.is_empty() {
            assert_eq!(reason_for_error(r.error_code).map(|x| x.code), Some(r.code));
        }
    }
}

#[tokio::test]
async fn dead_letters_description_lists_every_reason_code() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Edead AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tools = client.tools_list().await.expect("tools/list");
    let description = tools["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|t| t["name"] == "host.trigger.dead_letters")
        .and_then(|t| t["description"].as_str())
        .expect("host.trigger.dead_letters is listed")
        .to_string();
    for r in DEAD_LETTER_REASONS {
        assert!(description.contains(r.code), "description omits reason '{}'", r.code);
    }
    for code in constructed_codes() {
        let covered = DEAD_LETTER_REASONS
            .iter()
            .any(|r| r.error_code == code && description.contains(r.code));
        assert!(covered, "description does not list a reason for '{code}'");
    }
}
