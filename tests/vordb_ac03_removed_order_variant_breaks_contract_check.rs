//! PRD-mcphost-declared-ordering-per-verb-b
//! AC3 (P1) -- Given a PR that removes `NameAsc` from `DocsOrder`, When
//! `mcphost contract dump --check` runs, Then it fails as a breaking change
//! unless `deprecations.json` announces it.

use mcphost::api_contract::{Deprecation, Violation, diff, dump_contract};
use mcphost::kinds::KindRegistry;
use mcphost::paged::{PAGED_VERBS, paged_spec};
use serde_json::{Value, json};
use std::process::Command;

fn order_enum_mut<'a>(contract: &'a mut Value, verb: &str) -> &'a mut Vec<Value> {
    contract["tools"]
        .as_array_mut()
        .expect("tools")
        .iter_mut()
        .find(|t| t["name"] == json!(verb))
        .unwrap_or_else(|| panic!("{verb} in contract"))["input_schema"]["properties"]["order"]["enum"]
        .as_array_mut()
        .expect("order enum")
}

fn announcement(path: &str) -> Deprecation {
    Deprecation {
        path: path.to_string(),
        since: "2026-10-11".to_string(),
        sunset: "2026-12-10".to_string(),
        replacement: "removed order variant".to_string(),
        narrowing_only: true,
    }
}

#[test]
fn removing_name_asc_fails_the_diff_unless_announced() {
    let before = dump_contract(&KindRegistry::with_builtin());
    let mut after = before.clone();
    order_enum_mut(&mut after, "host.docs.list").retain(|v| v != &json!("name_asc"));
    let now = mcphost::state::now_unix();
    let unannounced = diff(&before, &after, &[], now);
    assert!(
        unannounced.iter().any(|v| matches!(v, Violation::Narrowed { path, .. } if path == "host.docs.list.order")),
        "{unannounced:?}"
    );
    assert!(diff(&before, &after, &[announcement("host.docs.list.order")], now).is_empty());
}

#[test]
fn every_declared_variant_of_every_paged_verb_is_protected() {
    let before = dump_contract(&KindRegistry::with_builtin());
    let now = mcphost::state::now_unix();
    for verb in PAGED_VERBS {
        let spec = paged_spec(verb).unwrap_or_else(|| panic!("{verb} has a spec"));
        for variant in (spec.orders)() {
            let mut after = before.clone();
            order_enum_mut(&mut after, verb).retain(|v| v != &json!(variant));
            assert!(!diff(&before, &after, &[], now).is_empty(), "removing {variant} from {verb} must be breaking");
        }
    }
}

#[test]
fn cli_check_names_the_breaking_change_and_honours_deprecations_json() {
    let dir = std::env::temp_dir().join(format!("vordb-ac03-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tmp dir");
    let mut committed = dump_contract(&KindRegistry::with_builtin());
    order_enum_mut(&mut committed, "host.docs.list").push(json!("name_desc"));
    std::fs::write(dir.join("host-tools.v1.json"), serde_json::to_string_pretty(&committed).expect("json")).expect("write");
    let run = || {
        let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
            .args(["contract", "dump", "--check", "--path"])
            .arg(dir.join("host-tools.v1.json"))
            .output()
            .expect("run mcphost");
        (out.status.success(), String::from_utf8_lossy(&out.stderr).into_owned())
    };
    let (ok, stderr) = run();
    assert!(!ok && stderr.contains("breaking change") && stderr.contains("host.docs.list.order"), "{stderr}");

    std::fs::write(dir.join("deprecations.json"), serde_json::to_string(&json!([announcement("host.docs.list.order")])).expect("json")).expect("ledger");
    let (_ok, stderr) = run();
    assert!(!stderr.contains("breaking change"), "announced removal is not breaking: {stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}
