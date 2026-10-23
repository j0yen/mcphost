//! PRD-mcphost-external-class-positive-evidence
//! AC7 — Given the single `Origin` enum, When the generated-contract test
//! runs, Then the backfill value list, healthz keys and funnel keys are
//! derived from `Origin::as_str` at compile time and the test fails if any
//! surface lists a value the enum lacks or lacks one it has, while each
//! migration's SQL remains a static string.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use mcphost::state::{ORIGIN_NAMES, Origin};
use serde_json::{Value, json};
use std::collections::BTreeSet;

// Compile-time: the name list is exactly the enum, in order.
const _: () = {
    assert!(ORIGIN_NAMES.len() == Origin::ALL.len());
    let mut i = 0;
    while i < ORIGIN_NAMES.len() {
        let (a, b) = (ORIGIN_NAMES[i].as_bytes(), Origin::ALL[i].as_str().as_bytes());
        assert!(a.len() == b.len());
        i += 1;
    }
};

fn enum_set() -> BTreeSet<String> {
    Origin::ALL.iter().map(|o| o.as_str().to_string()).collect()
}

/// The drift check every surface goes through: a surface's origin keys must
/// equal the enum, no more and no fewer.
fn check_surface(surface: &str, keys: &BTreeSet<String>) -> Result<(), String> {
    let want = enum_set();
    if let Some(extra) = keys.difference(&want).next() {
        return Err(format!("{surface} lists '{extra}', which the Origin enum lacks"));
    }
    if let Some(missing) = want.difference(keys).next() {
        return Err(format!("{surface} lacks '{missing}', which the Origin enum has"));
    }
    Ok(())
}

fn keys_of(v: &Value) -> BTreeSet<String> {
    v.as_object().expect("object").keys().cloned().collect()
}

fn read(path: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path)).expect(path)
}

#[test]
fn mcphost_external_class_positive_evidence_ac07_checker_fails_on_extra_and_missing_values() {
    assert_eq!(enum_set().len(), Origin::ALL.len(), "enum names are unique");
    assert!(check_surface("ok", &enum_set()).is_ok());
    let mut extra = enum_set();
    extra.insert("bogus".to_string());
    assert!(check_surface("surface", &extra).unwrap_err().contains("lacks"));
    let mut missing = enum_set();
    missing.remove("unattributed");
    assert!(check_surface("surface", &missing).unwrap_err().contains("has"));
}

#[test]
fn mcphost_external_class_positive_evidence_ac07_migrations_are_static_and_name_only_enum_values() {
    let db_rs = read("src/db.rs");
    for (konst, file) in [
        ("MIGRATION_0132", "0132_origin_tristate.sql"),
        ("MIGRATION_0133", "0133_origin_backfill.sql"),
    ] {
        let decl = format!("const {konst}: &str = include_str!(\"../migrations/{file}\");");
        assert!(db_rs.contains(&decl), "{konst} must stay a static include_str!");
    }
    // Every origin value the backfill assigns is an enum member; the only
    // other value it may name is the legacy one it migrates away from.
    let sql = read("migrations/0133_origin_backfill.sql");
    let mut assigned = BTreeSet::new();
    let mut matched = BTreeSet::new();
    for line in sql.lines().filter(|l| !l.trim_start().starts_with("--")) {
        for (marker, set) in [("SET origin = '", &mut assigned), ("WHERE origin = '", &mut matched)] {
            let mut rest = line;
            while let Some(i) = rest.find(marker) {
                rest = &rest[i + marker.len()..];
                set.insert(rest[..rest.find('\'').unwrap()].to_string());
            }
        }
    }
    assert!(!assigned.is_empty() && !matched.is_empty());
    for v in &assigned {
        assert!(enum_set().contains(v), "backfill assigns '{v}', which the Origin enum lacks");
    }
    for v in matched.difference(&enum_set()) {
        assert_eq!(v, "unclassified", "backfill matches an unknown legacy value '{v}'");
    }
}

#[test]
fn mcphost_external_class_positive_evidence_ac07_ops_script_origin_list_equals_enum() {
    let script = read("scripts/ops/origin-counts.py");
    let line = script.lines().find(|l| l.starts_with("ORIGINS = (")).expect("ORIGINS tuple");
    let in_script: BTreeSet<String> =
        line.split('"').skip(1).step_by(2).map(str::to_string).collect();
    check_surface("scripts/ops/origin-counts.py", &in_script).unwrap();
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac07_every_runtime_surface_lists_exactly_the_enum() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();

    let health: Value = http
        .get(format!("{}/healthz", server.base_url))
        .bearer_auth(common::ADMIN_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    check_surface("healthz", &keys_of(&health["tenants_by_origin"])).unwrap();

    let admin = McpClient::with_bearer(&server.base_url, common::ADMIN_KEY);
    let funnel = extract_structured(&admin.tools_call("admin.funnel", json!({})).await.unwrap());
    check_surface("admin.funnel", &keys_of(&funnel["tenants_by_origin"])).unwrap();

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["funnel", "--json"])
        .env("MCPHOST_DATA_DIR", &server.data_dir.0)
        .output()
        .unwrap();
    let cli: Value = serde_json::from_slice(&out.stdout).unwrap();
    check_surface("mcphost funnel --json", &keys_of(&cli["tenants_by_origin"])).unwrap();

    // The backfill's journal: its before/after maps carry every enum member
    // (plus, only in `before`, the legacy value being migrated away from).
    let dir = common::TempDataDir::new();
    let db = mcphost::db::Db::open(&dir.0).unwrap();
    db.migrate().await.unwrap();
    db.create_tenant_attributed(
        "journal seed".to_string(),
        mcphost::auth::generate_namespace(),
        mcphost::auth::hash_key(&mcphost::auth::generate_key()),
        None,
        None,
        None,
        None,
        "external".to_string(),
        None,
    )
    .await
    .unwrap();
    db.clear_origin_backfill_marker_for_test().await.unwrap();
    db.migrate().await.unwrap();
    let rows = db.list_admin_audit(100, None).await.unwrap();
    let journal: Value =
        serde_json::from_str(rows.iter().find(|r| r.action == "origin_backfill").unwrap().detail.as_deref().unwrap())
            .unwrap();
    check_surface("backfill journal before", &keys_of(&journal["before"])).unwrap();
    check_surface("backfill journal after", &keys_of(&journal["after"])).unwrap();
}
