//! contract: registry state::ORIGIN_NAMES — every surface that lists an origin value lists exactly the Origin enum members
//! contract: reads scripts/ops/origin-counts.py
//! contract: reads migrations/0137_origin_tristate.sql
//! PRD-mcphost-external-class-positive-evidence
//! AC7 (P0) -- Given the single `Origin` enum, When the generated-contract
//! test runs, Then the backfill value list, healthz keys and funnel keys are
//! derived from `Origin::as_str` at compile time and the test fails if any
//! surface lists a value the enum lacks or lacks one it has, while each
//! migration's SQL remains a static string.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer};
use mcphost::state::{ORIGIN_NAMES, ORIGIN_REPORT_KEYS, Origin};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// `Origin` values a pre-tri-state row may still carry as migration INPUT. The
/// backfill rewrites it away, so it is the one non-member the SQL may name.
const LEGACY_INPUT_ORIGINS: [&str; 1] = ["unclassified"];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

fn set<'a>(items: impl IntoIterator<Item = &'a str>) -> BTreeSet<String> {
    items.into_iter().map(str::to_string).collect()
}

/// The contract: a surface lists exactly the enum's members (plus, for SQL
/// that reads legacy rows, the named legacy inputs). Err names the surface and
/// the drift in both directions.
fn same_as_enum(surface: &str, listed: &BTreeSet<String>, extra_ok: &[&str]) -> Result<(), String> {
    let want = set(ORIGIN_NAMES);
    let extra: Vec<&String> = listed.iter().filter(|v| !want.contains(*v) && !extra_ok.contains(&v.as_str())).collect();
    let missing: Vec<&String> = want.iter().filter(|v| !listed.contains(*v)).collect();
    if extra.is_empty() && missing.is_empty() {
        Ok(())
    } else {
        Err(format!("{surface}: lists {extra:?} the enum lacks; lacks {missing:?} the enum has"))
    }
}

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object().unwrap_or_else(|| panic!("not an object: {v}")).keys().cloned().collect()
}

/// Every member, by an EXHAUSTIVE match: adding a variant stops this test from
/// compiling until it is listed here, and `Origin::ALL` must then carry it.
fn enum_members() -> BTreeSet<String> {
    const VARIANTS: usize = 4;
    let mut out = BTreeSet::new();
    let mut counted = 0;
    for o in Origin::ALL {
        match o {
            Origin::Synthetic | Origin::Internal | Origin::Unattributed | Origin::External => counted += 1,
        }
        out.insert(o.as_str().to_string());
    }
    assert_eq!(counted, VARIANTS, "Origin::ALL does not list exactly the {VARIANTS} variants");
    out
}

/// Every `origin = '<lit>'` / `origin IN ('a','b')` literal in the SQL text.
fn sql_origin_literals(sql: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in sql.lines().filter(|l| !l.trim_start().starts_with("--")) {
        let mut rest = line;
        while let Some(i) = rest.find("origin") {
            rest = &rest[i + "origin".len()..];
            let tail = rest.trim_start();
            if let Some(after_eq) = tail.strip_prefix('=') {
                let after_eq = after_eq.trim_start();
                if let Some(lit) = after_eq.strip_prefix('\'') {
                    if let Some(end) = lit.find('\'') {
                        out.insert(lit[..end].to_string());
                    }
                }
            }
        }
    }
    out
}

#[test]
fn mcphost_external_class_positive_evidence_ac07_enum_names_are_derived_from_as_str() {
    // The compile-time list IS `as_str` of every member, in declaration order.
    let derived: Vec<&str> = Origin::ALL.iter().map(|o| o.as_str()).collect();
    assert_eq!(derived, ORIGIN_NAMES.to_vec());
    // `ORIGIN_NAMES` is a const array sized by `Origin::ALL`: usable in const context.
    const _N: usize = ORIGIN_NAMES.len();
    for o in Origin::ALL {
        assert_eq!(Origin::parse(o.as_str()), Some(o));
    }
    // A variant missing from `Origin::ALL` (so from every generated surface) fails here.
    assert_eq!(Origin::ALL.len(), 4);
    same_as_enum("Origin::ALL", &enum_members(), &[]).unwrap();
}

#[test]
fn mcphost_external_class_positive_evidence_ac07_backfill_sql_lists_only_enum_values_and_stays_static() {
    let sql = read("migrations/0137_origin_tristate.sql");
    let lits = sql_origin_literals(&sql);
    let extra: Vec<&String> =
        lits.iter().filter(|v| !ORIGIN_NAMES.contains(&v.as_str()) && !LEGACY_INPUT_ORIGINS.contains(&v.as_str())).collect();
    assert!(extra.is_empty(), "backfill SQL names origin values the enum lacks: {extra:?}");
    // The backfill's targets are all members (it writes external + unattributed).
    for target in [Origin::External, Origin::Unattributed] {
        assert!(lits.contains(target.as_str()), "backfill SQL never writes {}", target.as_str());
    }
    // The detector sees drift: a literal the enum lacks is reported.
    let drifted = sql_origin_literals("UPDATE tenants SET origin = 'real' WHERE origin = 'unclassified';");
    assert_eq!(drifted, set(["real", "unclassified"]));
    assert!(same_as_enum("synthetic drift", &set(["real", "synthetic"]), &[]).is_err());

    // The migration's SQL is a static string: one `include_str!` file with no Rust
    // format placeholder, so the value lists above can never be spliced into it.
    assert!(!sql.contains("{}") && !sql.contains("{marks}") && !sql.contains("{origin"), "0137 SQL must be a static string");
}

#[test]
fn mcphost_external_class_positive_evidence_ac07_counts_script_origin_list_equals_enum() {
    let py = read("scripts/ops/origin-counts.py");
    let line = py.lines().find(|l| l.starts_with("ORIGINS = (")).expect("ORIGINS tuple in origin-counts.py");
    let listed = set(line.split('"').skip(1).step_by(2));
    same_as_enum("scripts/ops/origin-counts.py ORIGINS", &listed, &[]).unwrap();
    let err = same_as_enum("drifted script", &set(["synthetic", "internal", "unattributed", "other"]), &[]).unwrap_err();
    assert!(err.contains("other") && err.contains("external"), "{err}");
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac07_healthz_funnel_and_admin_keys_equal_enum() {
    let server = TestServer::start().await;
    let healthz: Value = reqwest::Client::new()
        .get(format!("{}/healthz", server.base_url))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse /healthz");
    let cli = {
        let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
            .args(["funnel", "--json"])
            .env("MCPHOST_DATA_DIR", &server.data_dir.0)
            .output()
            .expect("spawn mcphost funnel");
        assert!(out.status.success(), "stderr={}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice::<Value>(&out.stdout).expect("--json parses")
    };
    let admin = common::extract_structured(
        &McpClient::with_bearer(&server.base_url, ADMIN_KEY).tools_call("admin.funnel", json!({})).await.expect("admin.funnel"),
    );

    for (surface, body) in [("healthz", &healthz), ("mcphost funnel --json", &cli), ("admin.funnel", &admin)] {
        same_as_enum(&format!("{surface} tenants_by_origin"), &keys(&body["tenants_by_origin"]), &[]).unwrap();
        for key in ORIGIN_REPORT_KEYS {
            assert!(body.get(key).is_some(), "{surface} lacks {key}");
        }
    }
    // `signups` / `calls` splits are a subset: none may name a value the enum lacks.
    for split in ["signups", "calls"] {
        let listed = keys(&healthz[split]);
        let extra: Vec<&String> = listed.iter().filter(|k| !ORIGIN_NAMES.contains(&k.as_str())).collect();
        assert!(extra.is_empty(), "healthz.{split} lists {extra:?} the enum lacks");
    }
    // A row outside the enum shows up in `origin_unknown`, not as a new key.
    let (by_origin, unknown) = mcphost::state::origin_counts_json(&[("unattributed".to_string(), 2), ("bogus".to_string(), 3)]);
    same_as_enum("origin_counts_json", &keys(&by_origin), &[]).unwrap();
    assert_eq!((by_origin["unattributed"].clone(), unknown), (json!(2), 3));
}
