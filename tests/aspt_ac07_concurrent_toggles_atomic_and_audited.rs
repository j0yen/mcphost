//! AC7 (PRD-mcphost-admin-signup-pause-tool) — Given 20 concurrent admin
//! calls, 10 with distinct messages and 10 `resume: true`, When all complete,
//! Then every call succeeded, every `status()` read during the run was
//! not-paused or one of the 10 messages (never a truncated file's default
//! text), the final state agrees with healthz, and `admin.audit_log` holds 10
//! `signup_pause` rows (`detail` = message) and 10 `signup_resume` rows, each
//! with an `actor_key_id`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn aspt_ac07_twenty_concurrent_calls_never_expose_a_partial_file_and_are_audited() {
    let server = TestServer::start().await;
    let messages: BTreeSet<String> = (0..10).map(|i| format!("concurrent pause {i}")).collect();

    let done = Arc::new(AtomicBool::new(false));
    let reader = {
        let pause = server.state.signup_pause.clone();
        let done = done.clone();
        let allowed = messages.clone();
        tokio::spawn(async move {
            let mut reads = 0u64;
            let mut bad = Vec::new();
            while !done.load(Ordering::Relaxed) {
                if let Some(status) = pause.status()
                    && !allowed.contains(&status.message)
                {
                    bad.push(status.message);
                }
                reads += 1;
                tokio::task::yield_now().await;
            }
            (reads, bad)
        })
    };

    let mut calls = Vec::new();
    for (i, message) in messages.iter().cloned().enumerate() {
        for args in [json!({"message": message}), json!({"resume": true})] {
            let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
            calls.push(tokio::spawn(async move {
                // Interleave the two kinds across the start order.
                tokio::time::sleep(std::time::Duration::from_millis((i as u64 % 3) * 2)).await;
                admin.tools_call("admin.signup_pause", args).await
            }));
        }
    }
    for call in calls {
        call.await.expect("task").expect("every concurrent call succeeds");
    }
    done.store(true, Ordering::Relaxed);
    let (reads, bad) = reader.await.expect("reader task");
    assert!(reads > 0);
    assert!(bad.is_empty(), "status() saw text outside the 10 messages: {bad:?}");

    let health: Value = reqwest::Client::new()
        .get(format!("{}/healthz", server.base_url))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse /healthz");
    match server.state.signup_pause.status() {
        None => {
            assert_eq!(health["signups_enabled"], json!(true), "{health:?}");
            assert!(health.get("signup_pause_message").is_none(), "{health:?}");
        }
        Some(status) => {
            assert!(messages.contains(&status.message), "{}", status.message);
            assert_eq!(health["signups_enabled"], json!(false), "{health:?}");
            assert_eq!(health["signup_pause_message"], json!(status.message), "{health:?}");
        }
    }

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let log = admin.tools_call("admin.audit_log", json!({"limit": 100})).await.expect("admin.audit_log");
    let entries = extract_structured(&log)["entries"].as_array().expect("entries").clone();
    let pauses: Vec<&Value> = entries.iter().filter(|e| e["action"] == "signup_pause").collect();
    let resumes: Vec<&Value> = entries.iter().filter(|e| e["action"] == "signup_resume").collect();
    assert_eq!(pauses.len(), 10, "{entries:?}");
    assert_eq!(resumes.len(), 10, "{entries:?}");
    let logged: BTreeSet<String> = pauses.iter().map(|e| e["detail"].as_str().expect("detail").to_string()).collect();
    assert_eq!(logged, messages);
    for row in pauses.iter().chain(resumes.iter()) {
        assert!(row["actor_key_id"].as_str().is_some_and(|s| !s.is_empty()), "{row:?}");
    }
}
