//! PRD-mcphost-session-journal-v2
//! AC7 (P1) — Given 3 sessions in one window with 4, 6 and 2 calls (2 failed
//! in the second), When `host.calls.sessions` runs, Then 3 rows return newest
//! first with those `calls` counts and `failed: 2` on the second, and the verb
//! is present in the paged-verb tables and the generated contract
//! (`host-tools.v1.json`, `gendocs`).

use crate::common::{McpClient, TestServer, extract_structured, signup};
use mcphost::calllog;
use serde_json::{Value, json};
use std::path::Path;

async fn record(server: &TestServer, tenant_id: i64, session: &str, ok: bool) {
    let db = server.state.db.clone();
    calllog::with_session(Some(session.to_string()), async move {
        db.record_call_traced(
            tenant_id,
            "roll_echo".to_string(),
            2,
            ok,
            (!ok).then(|| "tool_exception".to_string()),
            None,
            None,
            if ok { "ok" } else { "error" },
            "external".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            "key".to_string(),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("record call");
    })
    .await;
}

#[tokio::test]
async fn jrnl_ac07_sessions_roll_up_newest_first() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Rollup Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");

    // Oldest first: s1 has 4 calls, s2 has 6 (2 failed), s3 has 2.
    for (session, calls, failed) in [("sess-one", 4, 0), ("sess-two", 6, 2), ("sess-three", 2, 0)] {
        for i in 0..calls {
            record(&server, tenant.id, session, i < calls - failed).await;
        }
    }
    // A call made without a session is not a session.
    server
        .state
        .db
        .record_call_traced(
            tenant.id, "roll_echo".to_string(), 2, true, None, None, None, "ok", "external".to_string(), None, None,
            None, None, None, None, "key".to_string(), None, None, None, None,
        )
        .await
        .expect("sessionless call");

    let out = extract_structured(&client.tools_call("host.calls.sessions", json!({"since": "-1h"})).await.expect("host.calls.sessions"));
    let rows = out["sessions"].as_array().expect("sessions");
    assert_eq!(rows.len(), 3, "{out}");
    let ids: Vec<&str> = rows.iter().map(|r| r["session_id"].as_str().expect("session_id")).collect();
    assert_eq!(ids, ["sess-three", "sess-two", "sess-one"], "newest first: {out}");
    let calls: Vec<i64> = rows.iter().map(|r| r["calls"].as_i64().expect("calls")).collect();
    assert_eq!(calls, [2, 6, 4], "{out}");
    let failed: Vec<i64> = rows.iter().map(|r| r["failed"].as_i64().expect("failed")).collect();
    assert_eq!(failed, [0, 2, 0], "{out}");
    assert_eq!(rows[1]["ok"], json!(4), "{out}");
    assert_eq!(rows[1]["tools"], json!(["roll_echo"]), "{out}");
    for key in ["session_id", "first_ts", "last_ts", "calls", "ok", "failed", "tools"] {
        assert!(rows[0].get(key).is_some(), "row carries {key}: {out}");
    }

    // `limit` caps the rows, and is clamped at 20.
    let one = extract_structured(&client.tools_call("host.calls.sessions", json!({"since": "-1h", "limit": 1})).await.expect("limit 1"));
    assert_eq!(one["sessions"].as_array().expect("sessions").len(), 1, "{one}");
    // Another tenant's list holds none of these sessions.
    let (_other_ns, other_key) = signup(&server.base_url, "Other Rollup Tenant").await;
    let other = McpClient::with_bearer(&server.base_url, &other_key);
    let theirs = extract_structured(&other.tools_call("host.calls.sessions", json!({"since": "-1h"})).await.expect("other"));
    assert_eq!(theirs["sessions"], json!([]), "{theirs}");
}

#[test]
fn jrnl_ac07_verb_is_in_the_paged_tables_and_generated_contract() {
    let verb = "host.calls.sessions";
    assert!(
        mcphost::paged::PAGED_VERBS.contains(&verb) || mcphost::paged::UNPAGED_VERBS.iter().any(|(v, _)| *v == verb),
        "{verb} is in the paged-verb tables"
    );
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let contract: Value =
        serde_json::from_slice(&std::fs::read(root.join("contracts/host-tools.v1.json")).expect("read contract")).expect("parse");
    assert!(contract["tools"].as_array().expect("tools").iter().any(|t| t["name"] == verb), "{verb} is in host-tools.v1.json");
    let docs = std::fs::read_to_string(root.join("docs/tools.md")).expect("read docs/tools.md");
    assert!(docs.contains(&format!("`{verb}`")), "{verb} is in the generated docs/tools.md");
}
