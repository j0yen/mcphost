//! PRD-mcphost-event-trigger-from-webhook AC7 (P1) — Given
//! contract: every refusal code built in the hook path (literal or err_code! const in src/) is in DEAD_LETTER_REASONS
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

/// Every `.rs` file under `dir`, read into one string each.
fn read_rs_tree(dir: &std::path::Path, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display())) {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            read_rs_tree(&path, out);
        } else if path.extension().is_some_and(|x| x == "rs") {
            out.push(std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display())));
        }
    }
}

/// The wire code an `err_code!` const `name` declares, read off its
/// declaration (`pub NAME, code: "x"` or the arm form `pub NAME => "x"`) in
/// the `src/` tree. The registry declaration is the one source, so a refusal
/// emitted as `code: SOME_CONST` resolves here and cannot drift from it.
fn declared_code(tree: &[String], name: &str) -> String {
    let mut found = BTreeSet::new();
    for src in tree {
        let mut rest = src.as_str();
        while let Some(i) = rest.find(name) {
            let before_ok = !rest[..i].ends_with(|c: char| c.is_alphanumeric() || c == '_');
            let after = &rest[i + name.len()..];
            rest = after;
            if !before_ok || after.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
                continue;
            }
            let tail = after.trim_start();
            let lit = if let Some(t) = tail.strip_prefix("=>") {
                Some(t.trim_start())
            } else {
                tail.strip_prefix(',')
                    .and_then(|t| t.trim_start().strip_prefix("code:"))
                    .map(str::trim_start)
            };
            if let Some(lit) = lit.and_then(|t| t.strip_prefix('"')) {
                if let Some(j) = lit.find('"') {
                    found.insert(lit[..j].to_string());
                }
            }
        }
    }
    assert_eq!(found.len(), 1, "const {name} must be declared once by err_code!, found {found:?}");
    found.into_iter().next().expect("one declaration")
}

/// Every `code: "<literal>"` and `code: <ERR_CONST>` in `text`, resolved to
/// its wire code through the `err_code!` declarations in `tree`.
fn code_literals(text: &str, tree: &[String]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = text;
    while let Some(i) = rest.find("code:") {
        // `reason_code:` / `error_code:` are letter fields, not AppError codes.
        let is_field = rest[..i].ends_with(|c: char| c.is_alphanumeric() || c == '_');
        rest = rest[i + "code:".len()..].trim_start_matches(' ');
        if is_field {
            continue;
        }
        if let Some(lit) = rest.strip_prefix('"') {
            if let Some(j) = lit.find('"') {
                out.insert(lit[..j].to_string());
            }
            continue;
        }
        let path: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':').collect();
        let name = path.rsplit("::").next().unwrap_or("");
        let is_const = name.starts_with(|c: char| c.is_ascii_uppercase())
            && name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        if is_const {
            out.insert(declared_code(tree, name));
        }
    }
    out
}

/// The `AppError` codes the hook refusal path constructs, read off the
/// source of the functions `handle_hook` refuses through.
fn constructed_codes() -> BTreeSet<String> {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/hooks.rs")).expect("read hooks.rs");
    let mut tree = Vec::new();
    read_rs_tree(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut tree);
    let handle_hook = fn_body(&src, "async fn handle_hook(");
    let mut codes = BTreeSet::new();
    for sig in [
        "fn hook_not_found(",
        "fn signature_invalid(",
        "fn signature_invalid_reason(",
        "fn verify_stripe_style(",
        "fn verify_event(",
    ] {
        codes.extend(code_literals(fn_body(&src, sig), &tree));
    }
    codes.extend(code_literals(handle_hook, &tree));
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
