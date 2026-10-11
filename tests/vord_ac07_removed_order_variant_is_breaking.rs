//! PRD-mcphost-declared-ordering-per-verb
//! AC7 (P1) -- Given a PR that removes `NameAsc` from `DocsOrder`, When
//! `mcphost contract dump --check` runs, Then it fails as a breaking change
//! unless `deprecations.json` announces it.
//!
//! Two levels: the comparator (`api_contract::diff`) on the real contract with
//! `name_asc` taken out of `host.docs.list`'s `order` enum, and the CLI
//! (`contract dump --check`) against a committed contract that still carries a
//! variant the live build no longer has.

use mcphost::api_contract::{Deprecation, Violation, diff, dump_contract};
use mcphost::kinds::KindRegistry;
use serde_json::{Value, json};
use std::process::Command;

fn order_enum_mut<'a>(contract: &'a mut Value, verb: &str) -> &'a mut Vec<Value> {
    contract["tools"]
        .as_array_mut()
        .expect("tools")
        .iter_mut()
        .find(|t| t["name"] == json!(verb))
        .expect("verb")["input_schema"]["properties"]["order"]["enum"]
        .as_array_mut()
        .expect("order enum")
}

fn announcement(path: &str) -> Deprecation {
    Deprecation {
        path: path.to_string(),
        since: "2026-10-11".to_string(),
        sunset: "2026-12-10".to_string(),
        replacement: "PRD-mcphost-declared-ordering-per-verb: name_asc removed".to_string(),
        narrowing_only: true,
    }
}

#[test]
fn removing_name_asc_is_breaking_unless_announced() {
    let before = dump_contract(&KindRegistry::with_builtin());
    let mut after = before.clone();
    order_enum_mut(&mut after, "host.docs.list").retain(|v| v != &json!("name_asc"));
    let now = mcphost::state::now_unix();

    let unannounced = diff(&before, &after, &[], now);
    assert!(
        unannounced.iter().any(|v| matches!(v, Violation::Narrowed { path, .. } if path == "host.docs.list.order")),
        "a removed order variant must be flagged: {unannounced:?}"
    );
    let announced = diff(&before, &after, &[announcement("host.docs.list.order")], now);
    assert!(announced.is_empty(), "an announced removal passes: {announced:?}");

    // A too-short lead time does not count as an announcement.
    let mut short = announcement("host.docs.list.order");
    short.sunset = "2026-10-20".to_string();
    assert!(!diff(&before, &after, &[short], now).is_empty());
}

#[test]
fn a_changed_default_order_is_breaking_unless_announced() {
    let before = dump_contract(&KindRegistry::with_builtin());
    let mut after = before.clone();
    after["tools"]
        .as_array_mut()
        .expect("tools")
        .iter_mut()
        .find(|t| t["name"] == json!("host.docs.list"))
        .expect("verb")["input_schema"]["properties"]["order"]["default"] = json!("name_asc");
    let now = mcphost::state::now_unix();
    assert!(!diff(&before, &after, &[], now).is_empty(), "a changed default must be flagged");
    assert!(diff(&before, &after, &[announcement("host.docs.list.order")], now).is_empty());
}

fn check(dir: &std::path::Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["contract", "dump", "--check", "--path"])
        .arg(dir.join("host-tools.v1.json"))
        .output()
        .expect("run mcphost");
    (out.status.success(), String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
fn contract_dump_check_names_a_removed_variant_as_a_breaking_change() {
    let dir = std::env::temp_dir().join(format!("vord-ac07-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tmp dir");
    // The committed contract still offers a variant the live build dropped.
    let mut committed = dump_contract(&KindRegistry::with_builtin());
    order_enum_mut(&mut committed, "host.docs.list").push(json!("name_desc"));
    std::fs::write(dir.join("host-tools.v1.json"), serde_json::to_string_pretty(&committed).expect("json")).expect("write");

    let (ok, stderr) = check(&dir);
    assert!(!ok, "--check must fail");
    assert!(stderr.contains("breaking change"), "stderr: {stderr}");
    assert!(stderr.contains("host.docs.list.order"), "stderr names the path: {stderr}");

    // Announced in deprecations.json beside the contract: no longer breaking
    // (the file is merely stale, which is a different message).
    std::fs::write(
        dir.join("deprecations.json"),
        serde_json::to_string_pretty(&json!([announcement("host.docs.list.order")])).expect("json"),
    )
    .expect("write ledger");
    let (ok, stderr) = check(&dir);
    assert!(!ok, "a stale file still fails --check");
    assert!(!stderr.contains("breaking change"), "an announced removal is not breaking: {stderr}");
    assert!(stderr.contains("stale"), "stderr: {stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}
