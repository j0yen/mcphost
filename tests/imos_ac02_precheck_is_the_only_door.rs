//! PRD-mcphost-implicit-signup-mints-only-on-success
//! AC2 (P0) — Given the landed crate, When a `compile_fail,E0603` doc-test in
//! `src/precheck.rs` calls `mcphost::host_args::validate_host_args` and another
//! calls `mcphost::paged::refuse_fields_on_unprojected`, Then both fail to
//! compile as private items, while `precheck_tenant_call` and `precheck_args`
//! are the only exports, and admin dispatch and preauth reach validation only
//! through `precheck_args`.
//!
//! The doc-tests themselves run under `cargo test --doc precheck`; this test
//! pins that they exist and that no other code can reach the two checks.

use std::path::Path;
use std::process::Command;

/// `grep -rnF <needle> src` (file:line:text lines).
fn grep_src(needle: &str) -> String {
    let out = Command::new("grep")
        .args(["-rnF", needle, "src"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("grep");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn src(rel: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)).expect(rel)
}

#[test]
fn imos_ac02_the_two_checks_are_private_to_the_crate_and_precheck_has_two_exports() {
    let precheck = src("src/precheck.rs");
    assert_eq!(precheck.matches("```compile_fail,E0603").count(), 2, "two E0603 doc-tests");
    assert!(precheck.contains("mcphost::host_args::validate_host_args"));
    assert!(precheck.contains("mcphost::paged::refuse_fields_on_unprojected"));
    let exports: Vec<&str> = precheck.lines().filter(|l| l.starts_with("pub ")).collect();
    assert_eq!(exports.len(), 2, "precheck exports exactly two items: {exports:?}");
    assert!(exports.iter().any(|l| l.starts_with("pub fn precheck_tenant_call(")));
    assert!(exports.iter().any(|l| l.starts_with("pub fn precheck_args(")));
    for def in ["pub(crate) fn validate_host_args(", "pub(crate) fn refuse_fields_on_unprojected("] {
        assert!(grep_src(def).contains(def), "{def} is crate-private, not `pub`");
    }
}

#[test]
fn imos_ac02_only_precheck_calls_the_checks_and_admin_and_preauth_use_precheck_args() {
    // Every call site, then drop comment lines and precheck.rs.
    let hits = grep_src("host_args::validate_host_args(") + &grep_src("paged::refuse_fields_on_unprojected(");
    for hit in hits.lines() {
        let mut parts = hit.splitn(3, ':');
        let (file, _line, text) = (parts.next().unwrap_or(""), parts.next(), parts.next().unwrap_or(""));
        assert!(
            file == "src/precheck.rs" || text.trim_start().starts_with("//"),
            "a call outside precheck: {hit}"
        );
    }
    let handler = src("src/handler.rs");
    let admin = handler.split("async fn dispatch_admin_tool(").nth(1).expect("dispatch_admin_tool");
    assert!(admin[..admin.find("note_handler_run").expect("note")].contains("precheck::precheck_args("));
    let preauth = src("src/handler/preauth.rs");
    assert_eq!(preauth.matches("precheck::precheck_args(").count(), 2, "call_trace + doctor");
}
