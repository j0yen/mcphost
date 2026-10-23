//! PRD-mcphost-incident-banner-for-agents-b
//! AC3 — Given a banner was attached, When the request log line is
//! written, Then it contains `incident=<id>`.
//!
//! Drives the real request-log middleware (`http::protocol_version_and_log`)
//! over a one-route router whose handler builds the same host-fault HTTP
//! error `public_tool` serves. Captured through `common::tracecap` (thread-keyed
//! capture on the one global subscriber), same as
//! `sessbind_ac10_request_log_records_the_upgraded_status.rs`.

use crate::common;
use axum::{Router, middleware, routing::get};
use common::TestServer;
use common::tracecap::Capture;
use mcphost::errors::AppError;
use serde_json::{Value, json};

/// `fields` of the last "http request" line for `path`.
fn last_line(buf: &Capture, path: &str) -> Option<Value> {
    buf.json_lines()
        .into_iter()
        .filter(|v| v["fields"]["path"] == path)
        .map(|v| v["fields"].clone())
        .next_back()
}

async fn serve(server: &TestServer) -> String {
    let state = server.state.clone();
    let app = Router::new()
        .route(
            "/boom",
            get({
                let state = state.clone();
                move || {
                    let state = state.clone();
                    async move { mcphost::public_tool::app_error_response(&state, AppError::Storage("disk".into())) }
                }
            }),
        )
        .route(
            "/bad",
            get({
                let state = state.clone();
                move || {
                    let state = state.clone();
                    async move { mcphost::public_tool::app_error_response(&state, AppError::ToolNotFound("x".into())) }
                }
            }),
        )
        .layer(middleware::from_fn(mcphost::http::protocol_version_and_log));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(listener, app).await.ok() });
    format!("http://{addr}")
}

#[tokio::test]
async fn incbanb_ac03() {
    let server = TestServer::start().await;
    let base = serve(&server).await;
    let http = reqwest::Client::new();
    let buf = common::tracecap::capture_json();

    // No incident open: the line has no incident field.
    let r = http.get(format!("{base}/boom")).send().await.expect("get");
    assert_eq!(r.status(), 500);
    let line = last_line(&buf, "/boom").expect("log line");
    assert!(line.get("incident").is_none(), "no banner, no incident field: {line}");

    let opened = mcphost::admin::incident_open(
        &server.state,
        &json!({"title": "storage stall", "impact": "partial", "components": ["mcp"]}),
    )
    .await
    .expect("open incident");
    let id = opened["id"].as_i64().expect("id");

    // Banner attached: the line carries incident=<id>.
    let r = http.get(format!("{base}/boom")).send().await.expect("get");
    let body: Value = r.json().await.expect("json");
    assert_eq!(body["data"]["incident"]["id"], id);
    let line = last_line(&buf, "/boom").expect("log line");
    assert_eq!(line["incident"], id, "{line}");

    // A tenant-fault response while the incident is open attaches nothing.
    let _ = http.get(format!("{base}/bad")).send().await.expect("get");
    let line = last_line(&buf, "/bad").expect("log line");
    assert!(line.get("incident").is_none(), "{line}");
}
