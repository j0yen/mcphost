//! contract: reads registry state::ORIGIN_NAMES
//! contract: reads scripts/ops/origin-counts.py
//! PRD-mcphost-external-class-positive-evidence
//! AC6 (P0) — Given the deployed binary on an empty DB and on the seeded DB,
//! When `/healthz`, `mcphost funnel --json`, admin `admin.funnel` and
//! `scripts/ops/origin-counts.py` are read, Then all four report
//! `tenants_by_origin`, `external_by_evidence`, `tenants_real`,
//! `tenants_confirmed` and `origin_unknown` with identical keys (every key 0
//! on the empty DB), `tenants_real == tenants_by_origin.external`,
//! `tenants_confirmed == external_by_evidence.claimed`, `origin_unknown == 0`,
//! and the script's counts equal healthz.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer};
use mcphost::auth::{generate_key, generate_namespace, hash_key};
use mcphost::state::{ORIGIN_NAMES, ORIGIN_REPORT_KEYS};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::process::Command;

async fn healthz(server: &TestServer) -> Value {
    reqwest::Client::new()
        .get(format!("{}/healthz", server.base_url))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse /healthz")
}

fn cli(server: &TestServer) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["funnel", "--json"])
        .env("MCPHOST_DATA_DIR", &server.data_dir.0)
        .output()
        .expect("spawn mcphost funnel");
    assert!(out.status.success(), "stderr={}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).expect("--json parses")
}

async fn admin(server: &TestServer) -> Value {
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    common::extract_structured(&admin.tools_call("admin.funnel", json!({})).await.expect("admin.funnel"))
}

/// `key=value` pairs of the script's single stdout line.
fn script(server: &TestServer) -> BTreeMap<String, i64> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = Command::new("python3")
        .arg("-I")
        .arg(root.join("scripts/ops/origin-counts.py"))
        .env("MCPHOST_DATA_DIR", &server.data_dir.0)
        .output()
        .expect("run origin-counts.py");
    assert!(out.status.success(), "stderr={}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 1, "exactly one line: {stdout:?}");
    stdout
        .split_whitespace()
        .map(|kv| {
            let (k, v) = kv.split_once('=').expect("key=value");
            (k.to_string(), v.parse().expect("integer"))
        })
        .collect()
}

fn report(v: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for key in ORIGIN_REPORT_KEYS {
        out.insert(key.to_string(), v.get(key).cloned().unwrap_or_else(|| panic!("missing {key} in {v}")));
    }
    Value::Object(out)
}

fn assert_consistent(r: &Value) {
    assert_eq!(r["tenants_real"], r["tenants_by_origin"]["external"], "{r}");
    assert_eq!(r["tenants_confirmed"], r["external_by_evidence"]["claimed"], "{r}");
    assert_eq!(r["origin_unknown"], json!(0), "{r}");
    let keys: Vec<&str> = r["tenants_by_origin"].as_object().unwrap().keys().map(String::as_str).collect();
    let mut want = ORIGIN_NAMES.to_vec();
    want.sort_unstable();
    let mut got = keys.clone();
    got.sort_unstable();
    assert_eq!(got, want, "{r}");
}

fn assert_script_matches(s: &BTreeMap<String, i64>, h: &Value) {
    let n = |v: &Value| v.as_i64().unwrap();
    assert_eq!(s["external"], n(&h["tenants_by_origin"]["external"]));
    assert_eq!(s["unattributed"], n(&h["tenants_by_origin"]["unattributed"]));
    assert_eq!(s["synthetic"], n(&h["tenants_by_origin"]["synthetic"]));
    assert_eq!(s["internal"], n(&h["tenants_by_origin"]["internal"]));
    assert_eq!(s["claimed"], n(&h["external_by_evidence"]["claimed"]));
    assert_eq!(s["returned"], n(&h["external_by_evidence"]["returned"]));
    assert_eq!(s["unknown"], n(&h["origin_unknown"]));
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac06_four_surfaces_agree_empty_and_seeded() {
    let server = TestServer::start().await;

    // Empty DB: identical keys everywhere, every value 0.
    let h = report(&healthz(&server).await);
    let (c, a) = (report(&cli(&server)), report(&admin(&server).await));
    assert_eq!(h, c, "healthz vs cli");
    assert_eq!(h, a, "healthz vs admin.funnel");
    assert_consistent(&h);
    assert_eq!(h["tenants_real"], json!(0));
    assert_eq!(h["tenants_confirmed"], json!(0));
    assert_eq!(h["external_by_evidence"], json!({"claimed": 0, "returned": 0}));
    assert!(h["tenants_by_origin"].as_object().unwrap().values().all(|v| v == &json!(0)), "{h}");
    let s = script(&server);
    assert_script_matches(&s, &h);
    assert!(s.values().all(|v| *v == 0), "{s:?}");

    // Seeded DB: one of each kind, two claimed, one returned, two legacy.
    for (name, origin, detail) in [
        ("claimed one", "external", "claimed-email"),
        ("claimed two", "external", "claimed-email"),
        ("returned", "external", "returned:3d"),
        ("fresh", "unattributed", "no-positive-evidence"),
        ("old a", "unattributed", "legacy-external-unverified"),
        ("old b", "unattributed", "legacy-unclassified"),
        ("harness", "synthetic", "harness:unstamped"),
        ("perf-free", "internal", "perf-free"),
    ] {
        server
            .state
            .db
            .create_tenant_attributed(
                name.to_string(),
                generate_namespace(),
                hash_key(&generate_key()),
                None,
                Some("external".to_string()),
                None,
                None,
                origin.to_string(),
                Some(detail.to_string()),
            )
            .await
            .expect("seed");
    }
    let h = report(&healthz(&server).await);
    assert_eq!(h, report(&cli(&server)), "healthz vs cli");
    assert_eq!(h, report(&admin(&server).await), "healthz vs admin.funnel");
    assert_consistent(&h);
    assert_eq!(h["tenants_real"], json!(3));
    assert_eq!(h["tenants_confirmed"], json!(2));
    assert_eq!(h["external_by_evidence"], json!({"claimed": 2, "returned": 1}));
    assert_eq!(h["tenants_by_origin"]["unattributed"], json!(3));
    let s = script(&server);
    assert_script_matches(&s, &h);
    assert_eq!(s["legacy"], 2, "{s:?}");
}
