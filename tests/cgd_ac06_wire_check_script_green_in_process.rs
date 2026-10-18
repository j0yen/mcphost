//! PRD-mcphost-capabilities-generated-from-dispatch
//! AC6 (P0) -- Given `scripts/capabilities-wire-check.sh` and the in-process
//! server, When the script runs against it, Then it prints `methods=N
//! labelled=N bare=0` and exits 0 (N = the registry's unimplemented-row and
//! server-to-client methods); Given one row's method answered bare by a
//! stubbed server, Then it prints `bare=1` and exits 1.

use crate::common;
use axum::routing::{get, post};
use axum::{Json, Router};
use common::TestServer;
use mcphost::capabilities::CAPABILITIES;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// The methods the script must probe, read from the registry.
fn expected_methods() -> BTreeSet<&'static str> {
    let mut set = BTreeSet::new();
    for c in CAPABILITIES {
        if !c.implemented {
            set.extend(c.methods.iter().copied());
        }
        set.extend(c.server_to_client.iter().copied());
    }
    set
}

async fn run_script(base_url: String) -> (String, i32) {
    tokio::task::spawn_blocking(move || {
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/capabilities-wire-check.sh");
        let out = std::process::Command::new("bash").arg(script).arg(base_url).output().expect("run script");
        (String::from_utf8_lossy(&out.stdout).trim().to_string(), out.status.code().unwrap_or(-1))
    })
    .await
    .expect("join")
}

#[tokio::test]
async fn cgd_ac06_script_reports_every_method_labelled_against_the_in_process_server() {
    let server = TestServer::start().await;
    let n = expected_methods().len();
    let (stdout, rc) = run_script(server.base_url.clone()).await;
    assert_eq!(stdout, format!("methods={n} labelled={n} bare=0"), "rc={rc}");
    assert_eq!(rc, 0);
}

#[tokio::test]
async fn cgd_ac06_script_reports_bare_one_and_exits_1_for_a_stubbed_bare_answer() {
    let server = TestServer::start().await;
    let real: Value = reqwest::get(format!("{}/capabilities.json", server.base_url))
        .await
        .expect("GET")
        .json()
        .await
        .expect("JSON");
    let bare_method = *expected_methods().iter().next().expect("a method");
    let app = Router::new()
        .route("/capabilities.json", get(move || async move { Json(real.clone()) }))
        .route(
            "/mcp",
            post(move |body: String| async move {
                let m = serde_json::from_str::<Value>(&body).expect("json")["method"].as_str().unwrap_or_default().to_string();
                let mut err = json!({"code": -32601, "message": m});
                if m != bare_method {
                    err["data"] = json!({"help_url": "https://stub.test/help/capability_unsupported"});
                }
                Json(json!({"jsonrpc": "2.0", "id": 1, "error": err}))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let (stdout, rc) = run_script(format!("http://{addr}")).await;
    let n = expected_methods().len();
    assert_eq!(stdout, format!("methods={n} labelled={} bare=1", n - 1));
    assert_eq!(rc, 1);
}
