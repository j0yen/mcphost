//! PRD-mcphost-end-user-audit-and-revoke
//! AC8 (P1) -- Given u1 with 5 state rows, When
//! `host.enduser.export {subject: "u1"}` runs, Then the bundle at
//! `/exports/<run_id>` contains exactly those rows and u1's call history,
//! and nothing from u2.

use std::collections::HashMap;
use std::io::Read;
use std::time::{Duration, Instant};

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use flate2::read::GzDecoder;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;
use serde_json::json;
use tar::Archive;

#[derive(Serialize)]
struct AssertionClaims {
    sub: String,
    iat: i64,
    exp: i64,
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn sign_assertion(secret: &str, sub: &str) -> String {
    let now = now_unix();
    let claims = AssertionClaims { sub: sub.to_string(), iat: now, exp: now + 300 };
    encode(&Header::new(Algorithm::HS256), &claims, &EncodingKey::from_secret(secret.as_bytes()))
        .expect("sign assertion")
}

fn read_archive_entries(path: &std::path::Path) -> HashMap<String, Vec<u8>> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read archive {path:?}: {e}"));
    let decoder = GzDecoder::new(&bytes[..]);
    let mut archive = Archive::new(decoder);
    let mut out = HashMap::new();
    for entry in archive.entries().expect("tar entries") {
        let mut entry = entry.expect("tar entry");
        let entry_path = entry.path().expect("entry path").to_string_lossy().to_string();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).expect("read entry data");
        out.insert(entry_path, data);
    }
    out
}

async fn wait_for_done(client: &McpClient, run_id: &str) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got = extract_structured(
            &client.tools_call("host.runs.get", json!({"run_id": run_id})).await.expect("host.runs.get ok"),
        );
        if got["status"] == json!("done") || got["status"] == json!("error") {
            return got;
        }
        assert!(Instant::now() < deadline, "export never finished: {got:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn export_bundle_contains_exactly_one_subjects_data() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC8 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    for name in ["tool_u1", "tool_u2"] {
        client
            .tools_call(
                "host.tool_publish",
                json!({"name": name, "kind": "echo", "spec": {"schema": {"type": "object"}}}),
            )
            .await
            .unwrap_or_else(|e| panic!("publish {name} failed: {} {}", e.code, e.message));
    }

    let rotate = client
        .tools_call("host.enduser.assertion_secret_rotate", json!({}))
        .await
        .expect("assertion_secret_rotate ok");
    let secret = extract_structured(&rotate)["secret"].as_str().expect("secret string").to_string();

    let a1 = sign_assertion(&secret, "u1");
    client
        .tools_call(&format!("{ns}.tool_u1"), json!({"end_user_assertion": a1}))
        .await
        .unwrap_or_else(|e| panic!("u1's call must succeed: {} {}", e.code, e.message));
    let a2 = sign_assertion(&secret, "u2");
    client
        .tools_call(&format!("{ns}.tool_u2"), json!({"end_user_assertion": a2}))
        .await
        .unwrap_or_else(|e| panic!("u2's call must succeed: {} {}", e.code, e.message));

    let tenant = server.state.db.find_tenant_by_namespace(ns).await.unwrap().expect("tenant");
    for _ in 0..5 {
        server
            .state
            .db
            .state_row_insert(tenant.id, "mytable".to_string(), "{}".to_string(), "u1".to_string())
            .await
            .expect("seed u1 row");
    }
    server
        .state
        .db
        .state_row_insert(tenant.id, "mytable".to_string(), "{}".to_string(), "u2".to_string())
        .await
        .expect("seed u2 row");

    let enqueue = extract_structured(
        &client
            .tools_call("host.enduser.export", json!({"subject": "u1"}))
            .await
            .expect("host.enduser.export ok"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id field").to_string();
    let finished = wait_for_done(&client, &run_id).await;
    assert_eq!(finished["status"], json!("done"), "export failed: {finished:?}");
    let result = &finished["result"];
    assert!(result["download_url"].as_str().is_some(), "result: {result:?}");

    let archive_path = server.state.db.data_dir().join("exports").join(format!("{run_id}.tar.gz"));
    let entries = read_archive_entries(&archive_path);

    let state_row_entries: Vec<&String> =
        entries.keys().filter(|k| k.starts_with("state_rows/mytable/")).collect();
    assert_eq!(state_row_entries.len(), 5, "entries: {:?}", entries.keys());

    let calls_jsonl = String::from_utf8(entries["calls.jsonl"].clone()).expect("calls.jsonl utf8");
    assert!(calls_jsonl.contains("tool_u1"), "{calls_jsonl}");
    assert!(!calls_jsonl.contains("tool_u2"), "must contain nothing from u2: {calls_jsonl}");
    assert_eq!(calls_jsonl.lines().count(), 1, "{calls_jsonl}");
}
