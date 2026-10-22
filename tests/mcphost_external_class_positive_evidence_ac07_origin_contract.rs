//! PRD-mcphost-external-class-positive-evidence
//! AC7 — Given the single `Origin` enum, When the generated-contract test
//! runs, Then the backfill value list, healthz keys and funnel keys are
//! derived from `Origin::as_str` at compile time and the test fails if any
//! surface lists a value the enum lacks or lacks one it has, while each
//! migration's SQL remains a static string.

use crate::common;
use common::{ADMIN_KEY, TestServer};
use mcphost::state::Origin;
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn enum_names() -> BTreeSet<String> {
    Origin::ALL.iter().map(|o| o.as_str().to_string()).collect()
}

fn keys_of(v: &Value) -> BTreeSet<String> {
    v.as_object().expect("object").keys().cloned().collect()
}

/// `NAMES` is a compile-time const built from `as_str`, covers `ALL` exactly
/// once each, and `ordinal` is `ALL`'s index (so a new variant cannot be
/// added without extending all three).
#[test]
fn mcphost_external_class_positive_evidence_ac07_const_names_derive_from_as_str() {
    const NAMES: [&str; 4] = Origin::NAMES;
    assert_eq!(NAMES.len(), Origin::ALL.len());
    for (i, o) in Origin::ALL.iter().enumerate() {
        assert_eq!(o.ordinal(), i);
        assert_eq!(Origin::NAMES[i], o.as_str());
        assert_eq!(Origin::parse(o.as_str()), Some(*o));
    }
    assert_eq!(enum_names().len(), Origin::ALL.len(), "names are distinct");
    assert_eq!(Origin::parse("unclassified"), None);
}

/// Every origin literal the migration SQL writes or matches is an enum
/// member (`unclassified`, the 0012 placeholder, is the one legacy sentinel
/// it may match), and every enum member the SQL names is accounted for.
#[test]
fn mcphost_external_class_positive_evidence_ac07_migration_sql_is_static_and_in_the_enum() {
    let sql = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations/0132_origin_tristate.sql"))
        .expect("read migration");
    let code: String = sql.lines().filter(|l| !l.trim_start().starts_with("--")).collect::<Vec<_>>().join("\n");
    assert!(!code.contains("{}") && !code.contains("format!"), "migration SQL is a static string");

    let lit = regex::Regex::new(r"'([a-z_-]+)'").unwrap();
    let assign = regex::Regex::new(r"(?s)SET\s+origin\s*=\s*(.*?)(?:,\s*origin_detail|\s+WHERE)").unwrap();
    let cmp = regex::Regex::new(r"origin\s*=\s*'([a-z_-]+)'").unwrap();
    let mut written = BTreeSet::new();
    for m in assign.captures_iter(&code) {
        for l in lit.captures_iter(&m[1]) {
            written.insert(l[1].to_string());
        }
    }
    let mut matched: BTreeSet<String> = cmp.captures_iter(&code).map(|c| c[1].to_string()).collect();
    matched.remove("unclassified");
    let names = enum_names();
    assert!(!written.is_empty(), "the migration writes origin values");
    assert!(written.is_subset(&names), "SQL writes a value the enum lacks: {written:?} vs {names:?}");
    assert!(matched.is_subset(&names), "SQL matches a value the enum lacks: {matched:?} vs {names:?}");
}

/// Healthz, `mcphost funnel --json` and admin funnel list exactly the enum's
/// members under `tenants_by_origin`; the script prints each member.
#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac07() {
    let server = TestServer::start().await;
    let names = enum_names();

    let health: Value = reqwest::Client::new()
        .get(format!("{}/healthz", server.base_url))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(keys_of(&health["tenants_by_origin"]), names, "healthz keys");

    let admin = mcphost::admin::funnel(&server.state, &json!({"days": 7})).await.unwrap();
    assert_eq!(keys_of(&admin["tenants_by_origin"]), names, "admin funnel keys");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["funnel", "--json"])
        .env("MCPHOST_DATA_DIR", &server.data_dir.0)
        .output()
        .unwrap();
    let cli: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(keys_of(&cli["tenants_by_origin"]), names, "funnel --json keys");

    let script = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/ops/origin-counts.py")).unwrap();
    for n in &names {
        assert!(script.contains(&format!("\"{n}\"")), "origin-counts.py does not mention {n}");
    }

    // Seeded: every distinct origin in the DB is an enum member.
    for o in Origin::ALL {
        let key = mcphost::auth::generate_key();
        server
            .state
            .db
            .create_tenant_attributed(
                format!("contract {}", o.as_str()),
                mcphost::auth::generate_namespace(),
                mcphost::auth::hash_key(&key),
                None,
                None,
                None,
                None,
                o.as_str().to_string(),
                None,
            )
            .await
            .unwrap();
    }
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).unwrap();
    let mut stmt = conn.prepare("SELECT DISTINCT origin FROM tenants").unwrap();
    let distinct: BTreeSet<String> =
        stmt.query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
    assert!(distinct.is_subset(&names), "DB holds an origin the enum lacks: {distinct:?}");
    let report = server.state.db.origin_report().await.unwrap();
    assert_eq!(report["origin_unknown"], json!(0));
}
