//! PRD-mcphost-external-class-positive-evidence
//! AC6 — Given the binary on an empty DB and on the seeded DB, When
//! `/healthz`, `mcphost funnel --json`, admin funnel and
//! `scripts/ops/origin-counts.py` are read, Then all four report
//! `tenants_by_origin`, `external_by_evidence`, `tenants_real`,
//! `tenants_confirmed` and `origin_unknown` with identical keys (every key 0
//! on the empty DB), `tenants_real == tenants_by_origin.external`,
//! `tenants_confirmed == external_by_evidence.claimed`, `origin_unknown ==
//! 0`, and the script's counts equal healthz.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::process::Command;

const KEYS: [&str; 5] =
    ["tenants_by_origin", "external_by_evidence", "tenants_real", "tenants_confirmed", "origin_unknown"];

async fn healthz(server: &TestServer) -> Value {
    reqwest::Client::new()
        .get(format!("{}/healthz", server.base_url))
        .bearer_auth(common::ADMIN_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn admin_funnel(server: &TestServer) -> Value {
    let admin = McpClient::with_bearer(&server.base_url, common::ADMIN_KEY);
    extract_structured(&admin.tools_call("admin.funnel", json!({})).await.expect("admin.funnel"))
}

fn cli_funnel(server: &TestServer) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["funnel", "--json"])
        .env("MCPHOST_DATA_DIR", &server.data_dir.0)
        .output()
        .expect("run mcphost funnel --json");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).expect("funnel --json is JSON")
}

fn script_counts(server: &TestServer) -> BTreeMap<String, i64> {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/ops/origin-counts.py");
    let out = Command::new("python3")
        .arg("-I")
        .arg(&script)
        .env("MCPHOST_DATA_DIR", &server.data_dir.0)
        .output()
        .expect("run origin-counts.py");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let line = String::from_utf8(out.stdout).unwrap();
    assert_eq!(line.trim_end().lines().count(), 1, "exactly one line: {line:?}");
    line.split_whitespace()
        .map(|kv| {
            let (k, v) = kv.split_once('=').expect("key=value");
            (k.to_string(), v.parse().expect("integer"))
        })
        .collect()
}

/// The five shared keys of one surface, with the nested key sets.
fn shape(v: &Value) -> Value {
    let pick = |k: &str| v.get(k).unwrap_or_else(|| panic!("missing {k} in {v}")).clone();
    json!({
        "tenants_by_origin_keys": pick("tenants_by_origin").as_object().unwrap().keys().collect::<Vec<_>>(),
        "evidence_keys": pick("external_by_evidence").as_object().unwrap().keys().collect::<Vec<_>>(),
        "scalars": [pick("tenants_real").is_i64(), pick("tenants_confirmed").is_i64(), pick("origin_unknown").is_i64()],
    })
}

fn assert_invariants(v: &Value) {
    assert_eq!(v["tenants_real"], v["tenants_by_origin"]["external"], "{v}");
    assert_eq!(v["tenants_confirmed"], v["external_by_evidence"]["claimed"], "{v}");
    assert_eq!(v["origin_unknown"], json!(0), "{v}");
}

fn projection(v: &Value) -> Value {
    json!(KEYS.iter().map(|k| (k.to_string(), v[*k].clone())).collect::<BTreeMap<_, _>>())
}

async fn check_surfaces(server: &TestServer) -> Value {
    let (h, a, c) = (healthz(server).await, admin_funnel(server).await, cli_funnel(server));
    for v in [&h, &a, &c] {
        assert_invariants(v);
    }
    assert_eq!(shape(&h), shape(&a));
    assert_eq!(shape(&h), shape(&c));
    assert_eq!(projection(&h), projection(&a));
    assert_eq!(projection(&h), projection(&c));

    let s = script_counts(server);
    let get = |k: &str| h["tenants_by_origin"][k].as_i64().unwrap();
    assert_eq!(s["external"], h["tenants_real"].as_i64().unwrap());
    assert_eq!(s["claimed"], h["external_by_evidence"]["claimed"].as_i64().unwrap());
    assert_eq!(s["returned"], h["external_by_evidence"]["returned"].as_i64().unwrap());
    assert_eq!(s["unattributed"], get("unattributed"));
    assert_eq!(s["synthetic"], get("synthetic"));
    assert_eq!(s["internal"], get("internal"));
    assert_eq!(s["unknown"], h["origin_unknown"].as_i64().unwrap());
    h
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac06_surfaces_agree_on_empty_and_seeded_db() {
    let server = TestServer::start().await;

    // Empty DB: every key present and 0.
    let empty = check_surfaces(&server).await;
    assert_eq!(empty["tenants_real"], json!(0));
    assert_eq!(empty["tenants_confirmed"], json!(0));
    for (_, n) in empty["tenants_by_origin"].as_object().unwrap() {
        assert_eq!(n, &json!(0));
    }
    assert_eq!(empty["external_by_evidence"], json!({"claimed": 0, "returned": 0}));

    // Seeded: one claimed external, one returned external, two
    // unattributed, one synthetic (loopback), one internal.
    let now = mcphost::state::now_unix();
    let mut ids = Vec::new();
    for (i, ip) in ["203.0.113.51", "203.0.113.52", "203.0.113.53", "203.0.113.54"].iter().enumerate() {
        let r = mcphost::control::signup(
            &server.state,
            &json!({"name": format!("AC6 {i}")}),
            ip,
            mcphost::control::SignupAttribution::default(),
        )
        .await
        .unwrap();
        let t = server
            .state
            .db
            .find_tenant_by_namespace(r["tenant"].as_str().unwrap().to_string())
            .await
            .unwrap()
            .unwrap();
        ids.push(t.id);
    }
    server.state.db.verify_owner(ids[0], "dns".to_string(), "ac6.example".to_string()).await.unwrap();
    server
        .state
        .db
        .set_claim_timing_for_test(ids[1], "unattributed".to_string(), now - 3 * 86_400, None, None)
        .await
        .unwrap();
    server
        .state
        .db
        .observe_call_origin(ids[1], "unattributed", now - 3 * 86_400, now, false, "hash".to_string())
        .await
        .unwrap();
    common::signup(&server.base_url, "AC6 Loopback").await;
    server
        .state
        .db
        .create_tenant_attributed(
            "perf-free".to_string(),
            mcphost::auth::generate_namespace(),
            mcphost::auth::hash_key(&mcphost::auth::generate_key()),
            None,
            None,
            None,
            None,
            "internal".to_string(),
            Some("perf-free".to_string()),
        )
        .await
        .unwrap();

    let seeded = check_surfaces(&server).await;
    assert_eq!(seeded["tenants_by_origin"], json!({"synthetic": 1, "internal": 1, "unattributed": 2, "external": 2}));
    assert_eq!(seeded["external_by_evidence"], json!({"claimed": 1, "returned": 1}));
    assert_eq!(seeded["tenants_real"], json!(2));
    assert_eq!(seeded["tenants_confirmed"], json!(1));
    // Same keys on empty and seeded.
    assert_eq!(shape(&empty), shape(&seeded));
}
