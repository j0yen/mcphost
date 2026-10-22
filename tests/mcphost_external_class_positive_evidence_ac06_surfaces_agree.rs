//! PRD-mcphost-external-class-positive-evidence
//! AC6 — Given the binary on an empty DB and on a seeded DB, When `/healthz`,
//! `mcphost funnel --json`, admin funnel and `scripts/ops/origin-counts.py`
//! are read, Then all four report `tenants_by_origin`, `external_by_evidence`,
//! `tenants_real`, `tenants_confirmed` and `origin_unknown` with identical
//! keys (every key 0 on the empty DB), `tenants_real ==
//! tenants_by_origin.external`, `tenants_confirmed ==
//! external_by_evidence.claimed`, `origin_unknown == 0`, and the script's
//! counts equal healthz.

use crate::common;
use common::{ADMIN_KEY, TestServer};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const KEYS: [&str; 5] =
    ["tenants_by_origin", "external_by_evidence", "tenants_real", "tenants_confirmed", "origin_unknown"];

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

fn cli_funnel(server: &TestServer) -> Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["funnel", "--json"])
        .env("MCPHOST_DATA_DIR", &server.data_dir.0)
        .output()
        .expect("spawn mcphost funnel");
    assert!(output.status.success(), "stderr={}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).expect("--json parses")
}

fn script_counts(server: &TestServer) -> BTreeMap<String, i64> {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/ops/origin-counts.py");
    let output = std::process::Command::new("python3")
        .arg("-I")
        .arg(script)
        .env("MCPHOST_DATA_DIR", &server.data_dir.0)
        .output()
        .expect("spawn python3");
    assert!(output.status.success(), "stderr={}", String::from_utf8_lossy(&output.stderr));
    let line = String::from_utf8(output.stdout).unwrap();
    assert_eq!(line.trim_end_matches('\n').lines().count(), 1, "exactly one line: {line:?}");
    line.split_whitespace()
        .map(|kv| {
            let (k, v) = kv.split_once('=').expect("k=v");
            (k.to_string(), v.parse().expect("int"))
        })
        .collect()
}

fn shape(v: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for k in KEYS {
        assert!(v.get(k).is_some(), "missing {k} in {v}");
        out.insert(k.to_string(), v[k].clone());
    }
    Value::Object(out)
}

fn key_set(v: &Value) -> Vec<String> {
    fn walk(prefix: &str, v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(m) => {
                for (k, v) in m {
                    walk(&format!("{prefix}.{k}"), v, out);
                }
            }
            _ => out.push(prefix.to_string()),
        }
    }
    let mut out = Vec::new();
    walk("", &shape(v), &mut out);
    out.sort();
    out
}

async fn four_surfaces(server: &TestServer) -> [Value; 4] {
    let h = healthz(server).await;
    let cli = cli_funnel(server);
    let admin = mcphost::admin::funnel(&server.state, &json!({"days": 7})).await.expect("admin.funnel");
    let counts = script_counts(server);
    // The script prints flat counts; rebuild the shared shape from them.
    let script = json!({
        "tenants_by_origin": {
            "synthetic": counts["synthetic"], "internal": counts["internal"],
            "unattributed": counts["unattributed"], "external": counts["external"],
        },
        "external_by_evidence": {"claimed": counts["claimed"], "returned": counts["returned"]},
        "tenants_real": counts["external"],
        "tenants_confirmed": counts["claimed"],
        "origin_unknown": counts["unknown"],
    });
    [h, cli, admin, script]
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac06() {
    let server = TestServer::start().await;

    // Empty DB: identical keys, every value 0.
    let empty = four_surfaces(&server).await;
    let first = key_set(&empty[0]);
    for (i, s) in empty.iter().enumerate() {
        assert_eq!(key_set(s), first, "surface {i} key set differs");
        assert_eq!(shape(s), shape(&empty[0]), "surface {i} differs on the empty DB");
    }
    let flat: Vec<i64> = {
        let mut out = Vec::new();
        fn leaves(v: &Value, out: &mut Vec<i64>) {
            match v {
                Value::Object(m) => m.values().for_each(|v| leaves(v, out)),
                Value::Number(n) => out.push(n.as_i64().unwrap()),
                other => panic!("non-number leaf {other}"),
            }
        }
        leaves(&shape(&empty[0]), &mut out);
        out
    };
    assert!(flat.iter().all(|n| *n == 0), "every key is 0 on an empty DB: {flat:?}");

    // Seeded DB: one tenant per evidence kind.
    let conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let cases: [(&str, &str); 6] = [
        ("external", "claimed-email"),
        ("external", "returned:2d"),
        ("unattributed", "no-positive-evidence"),
        ("unattributed", "legacy-external-unverified"),
        ("synthetic", "harness:unstamped"),
        ("internal", "perf-free"),
    ];
    for (origin, detail) in cases {
        let key = mcphost::auth::generate_key();
        let t = server
            .state
            .db
            .create_tenant_attributed(
                format!("{origin} {detail}"),
                mcphost::auth::generate_namespace(),
                mcphost::auth::hash_key(&key),
                None,
                None,
                None,
                None,
                origin.to_string(),
                Some(detail.to_string()),
            )
            .await
            .expect("seed");
        conn.execute("UPDATE tenants SET origin_detail = ?1 WHERE id = ?2", params![detail, t.id]).unwrap();
    }
    let seeded = four_surfaces(&server).await;
    for (i, s) in seeded.iter().enumerate() {
        assert_eq!(key_set(s), first, "surface {i} key set differs when seeded");
        assert_eq!(shape(s), shape(&seeded[0]), "surface {i} differs when seeded: {s}");
        assert_eq!(s["tenants_real"], s["tenants_by_origin"]["external"]);
        assert_eq!(s["tenants_confirmed"], s["external_by_evidence"]["claimed"]);
        assert_eq!(s["origin_unknown"], json!(0));
    }
    assert_eq!(seeded[0]["tenants_by_origin"]["external"], json!(2));
    assert_eq!(seeded[0]["tenants_by_origin"]["unattributed"], json!(2));
    assert_eq!(seeded[0]["external_by_evidence"], json!({"claimed": 1, "returned": 1}));

    // The script's `legacy` count is the one legacy-* unattributed row.
    assert_eq!(script_counts(&server)["legacy"], 1);
}
