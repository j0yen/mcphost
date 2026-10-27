//! PRD-mcphost-table-import-from-url
//! AC2 (P0) — Given `source_url` values resolving to 127.0.0.1, 10.0.0.1,
//! 169.254.169.254, fd00::1, an http:// URL, and an https URL redirecting to
//! http, When `host.table.append` is called, Then each returns
//! `import_url_forbidden` naming the host and the fetch counter records zero
//! bytes received.
//!
//! The fetch counter is the tenant's import ledger (`bytes` per attempt). The
//! first five run against a state whose public url is not loopback (the
//! production policy); the redirect case needs a reachable first hop, which
//! only a loopback-hosted instance may fetch, so it runs on the harness's own
//! server, where the redirect hop itself is still https-only.

use crate::common;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::IntoResponse;
use common::sypul::{Fixture, orders_csv};
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::errors::AppError;
use mcphost::table::import::ledger;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

async fn redirector(location: String) -> (String, Arc<AtomicU64>) {
    let hits = Arc::new(AtomicU64::new(0));
    async fn go(State((location, hits)): State<(String, Arc<AtomicU64>)>) -> impl IntoResponse {
        hits.fetch_add(1, Ordering::SeqCst);
        let mut h = HeaderMap::new();
        h.insert(header::LOCATION, HeaderValue::from_str(&location).expect("location"));
        (StatusCode::FOUND, h)
    }
    let app = Router::new().fallback(go).with_state((location, hits.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind redirector");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}/start.csv"), hits)
}

fn forbidden(err: AppError) -> Value {
    match err {
        AppError::Structured { code, message, data } => {
            assert_eq!(code, "import_url_forbidden", "{message}");
            assert!(message.contains(data["host"].as_str().expect("data.host")), "message names the host: {message}");
            data
        }
        other => panic!("expected import_url_forbidden, got {other:?}"),
    }
}

#[tokio::test]
async fn tblurl_ac02_private_http_and_redirected_urls_are_refused_with_zero_bytes() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "TBLURL AC2").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let created = client.tools_call("host.table.create", json!({"name": "orders", "columns": {"id": "integer", "name": "text", "qty": "integer"}})).await.expect("create");
    assert_eq!(extract_structured(&created)["created"], json!(true));
    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.expect("lookup").expect("tenant");

    // The production policy: a non-loopback public url.
    let mut strict = (*server.state).clone();
    strict.public_url = "https://mcphost.example".to_string();
    let bait = Fixture::start(&orders_csv(10), false).await;

    let cases = [
        (bait.url("x.csv"), "127.0.0.1"),
        ("https://127.0.0.1/x.csv".to_string(), "127.0.0.1"),
        ("https://10.0.0.1/x.csv".to_string(), "10.0.0.1"),
        ("https://169.254.169.254/latest/meta-data/".to_string(), "169.254.169.254"),
        ("https://[fd00::1]/x.csv".to_string(), "fd00::1"),
        ("http://example.com/x.csv".to_string(), "example.com"),
        ("https://100.64.0.1/x.csv".to_string(), "100.64.0.1"),
        ("https://[::ffff:10.0.0.1]/x.csv".to_string(), "::ffff:a00:1"),
    ];
    for (url, host) in &cases {
        let err = mcphost::tables::table_append(&strict, &tenant, &json!({"table": "orders", "source_url": url}))
            .await
            .expect_err(url);
        let data = forbidden(err);
        assert_eq!(data["host"], json!(host), "{url}");
    }
    assert_eq!(bait.hits(), 0, "the loopback fixture was never contacted");

    // An https URL whose redirect lands on plain http (the loopback harness
    // server may fetch the first hop; the redirect hop is https-only).
    let target = Fixture::start(&orders_csv(10), false).await;
    let (start, redirect_hits) = redirector(target.csv_url()).await;
    let err = mcphost::tables::table_append(&server.state, &tenant, &json!({"table": "orders", "source_url": start}))
        .await
        .expect_err("redirect to http");
    let data = forbidden(err);
    assert_eq!(data["host"], json!("127.0.0.1"));
    assert_eq!(redirect_hits.load(Ordering::SeqCst), 1, "the first hop answered the redirect");
    assert_eq!(target.hits(), 0, "the http redirect target was never fetched");

    let rows = ledger(&server.state, &tenant).await.expect("ledger");
    assert_eq!(rows.len(), cases.len() + 1, "{rows:?}");
    for row in &rows {
        assert_eq!(row["bytes"], json!(0), "{row}");
        assert_eq!(row["outcome"], json!("import_url_forbidden"), "{row}");
        assert_eq!(row["rows_imported"], json!(0), "{row}");
    }
    let count = client.tools_call("host.table.count", json!({"table": "orders"})).await.expect("count");
    assert_eq!(extract_structured(&count)["count"], json!(0), "the table is unchanged");
}

#[test]
fn tblurl_ac02_at_most_three_https_redirects_to_public_hosts_are_followed() {
    use mcphost::net::fetch_guard::{FetchPolicy, hop_refusal};
    let policy = FetchPolicy::default();
    let public = reqwest::Url::parse("https://data.example.org/a.csv").unwrap();
    for visited in 1..=3 {
        assert!(hop_refusal(&public, visited, policy).is_none(), "redirect {visited} is followed");
    }
    let (host, reason) = hop_refusal(&public, 4, policy).expect("the fourth redirect is refused");
    assert_eq!(host, "data.example.org");
    assert!(reason.contains("3 redirects"), "{reason}");
    for next in ["http://data.example.org/a.csv", "https://10.1.2.3/a.csv", "https://169.254.169.254/", "https://[fd00::1]/"] {
        let url = reqwest::Url::parse(next).unwrap();
        assert!(hop_refusal(&url, 1, policy).is_some(), "{next}");
        // Even the dev policy keeps redirects https-only.
        assert!(hop_refusal(&url, 1, FetchPolicy { loopback_dev: true }).is_some(), "{next} (dev policy)");
    }
}
