//! PRD-mcphost-task-handoff-live-script AC5 (P0) — Given a successful run
//! against the local server, When `admin.tenants` is listed afterwards, Then
//! neither `handoff-live-a` nor `handoff-live-b` is an active tenant, and a
//! run whose final `wait` fails still offboards both.

use crate::common;
use common::{McpClient, extract_structured};
use crate::serve;
use serve::{ServeChild, ServeEnv};

use axum::{Router, body::Bytes, extract::State, http::{HeaderMap, StatusCode}, response::IntoResponse};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const ADMIN_KEY: &str = "thlive5-admin";

async fn start_server() -> (ServeChild, common::TempDataDir) {
    let data = common::TempDataDir::new();
    let mut server = ServeChild::spawn(ServeEnv {
        data_dir: data.0.clone(),
        admin_key: Some(ADMIN_KEY.into()),
        secret_key: Some("thlive5-secret".into()),
        extra_env: vec![("MCPHOST_LOG_LEVEL".into(), "warn".into())],
        ..ServeEnv::default()
    });
    server.wait_ready().await;
    (server, data)
}

async fn run_script(base: &str) -> std::process::Output {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/task-handoff-live.sh");
    tokio::process::Command::new("bash")
        .arg(script)
        .arg(base)
        .output()
        .await
        .expect("run script")
}

/// `(display_name, disabled)` for every tenant named `handoff-live-*`.
async fn handoff_live_tenants(base: &str) -> Vec<(String, bool)> {
    let admin = McpClient::with_bearer(base, ADMIN_KEY);
    let listed = extract_structured(&admin.tools_call("admin.tenants", json!({})).await.expect("admin.tenants"));
    listed["tenants"]
        .as_array()
        .unwrap_or_else(|| panic!("tenants array: {listed:?}"))
        .iter()
        .filter(|t| t["display_name"].as_str().is_some_and(|n| n.starts_with("handoff-live-")))
        .map(|t| (t["display_name"].as_str().unwrap().to_string(), t["disabled"].as_bool().unwrap_or(false)))
        .collect()
}

#[tokio::test]
async fn a_successful_run_leaves_neither_tenant_active() {
    let (server, _data) = start_server().await;
    let out = run_script(&server.base_url()).await;
    assert_eq!(out.status.code(), Some(0), "{out:?}");

    let tenants = handoff_live_tenants(&server.base_url()).await;
    let mut names: Vec<&str> = tenants.iter().map(|(n, _)| n.as_str()).collect();
    names.sort();
    assert_eq!(names, ["handoff-live-a", "handoff-live-b"], "{tenants:?}");
    assert!(tenants.iter().all(|(_, disabled)| *disabled), "both offboarded: {tenants:?}");
}

#[derive(Clone)]
struct Proxy {
    upstream: String,
    completed: Arc<AtomicBool>,
    http: reqwest::Client,
}

/// Forwards every request to the real server, except that once a
/// `host.task.complete` has gone through, any `host.msg.wait` gets a 500.
async fn proxy(State(p): State<Proxy>, headers: HeaderMap, body: Bytes) -> impl IntoResponse {
    let text = String::from_utf8_lossy(&body).to_string();
    if p.completed.load(Ordering::SeqCst) && text.contains("host.msg.wait") {
        return (StatusCode::INTERNAL_SERVER_ERROR, HeaderMap::new(), Bytes::from_static(b"boom"));
    }
    let mut req = p.http.post(format!("{}/mcp", p.upstream)).body(body.to_vec());
    for (k, v) in headers.iter() {
        let n = k.as_str();
        if n == "content-type" || n == "accept" || n == "authorization" || n.starts_with("mcp") {
            req = req.header(n, v);
        }
    }
    let resp = req.send().await.expect("forward");
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap();
    let mut out_headers = HeaderMap::new();
    if let Some(ct) = resp.headers().get("content-type") {
        out_headers.insert("content-type", ct.clone());
    }
    let bytes = resp.bytes().await.expect("body");
    if text.contains("host.task.complete") {
        p.completed.store(true, Ordering::SeqCst);
    }
    (status, out_headers, bytes)
}

#[tokio::test]
async fn a_run_whose_final_wait_fails_still_offboards_both() {
    let (server, _data) = start_server().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind proxy");
    let proxy_base = format!("http://{}", listener.local_addr().unwrap());
    let state = Proxy {
        upstream: server.base_url(),
        completed: Arc::new(AtomicBool::new(false)),
        http: reqwest::Client::new(),
    };
    let app = Router::new().route("/mcp", axum::routing::post(proxy)).with_state(state);
    let serving = tokio::spawn(async move { axum::serve(listener, app).await });

    let out = run_script(&proxy_base).await;
    serving.abort();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(stdout.starts_with("status: error reason: wait_a"), "{stdout:?}");

    let tenants = handoff_live_tenants(&server.base_url()).await;
    assert_eq!(tenants.len(), 2, "{tenants:?}");
    assert!(tenants.iter().all(|(_, disabled)| *disabled), "both offboarded: {tenants:?}");
}
