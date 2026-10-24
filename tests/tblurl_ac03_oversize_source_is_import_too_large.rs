//! PRD-mcphost-table-import-from-url
//! AC3 (P0) — Given a served CSV larger than the plan cap, When imported,
//! Then the result is `import_too_large` with `data.bytes` equal to the cap and
//! the table is unchanged.
//!
//! The cap is the plan's `table_bytes_max` (at most 25 MB); the test lowers the
//! free plan's to 64 KiB (an empty tenant table store already weighs 40 KiB).
//! Two servers: one that declares a Content-Length (refused before a body byte
//! is read) and one that streams without one (counted while it arrives,
//! abandoned one chunk past the cap).

use crate::common;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{HeaderValue, header};
use axum::response::Response;
use common::sypul::{Fixture, orders_csv};
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::table::import::ledger;
use serde_json::{Value, json};
use std::convert::Infallible;

const CAP: i64 = 65536;

async fn capped_server() -> TestServer {
    TestServer::start_with_plans(|catalog| {
        for p in &mut catalog.plans {
            if p.name == "free" {
                p.table_bytes_max = CAP;
            }
        }
    })
    .await
}

/// Streams 4 MB of CSV with no Content-Length.
async fn streaming_csv() -> String {
    async fn stream() -> Response {
        let header_line = Bytes::from_static(b"id,name,qty\n");
        let chunk = Bytes::from("1,aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa,2\n".repeat(80));
        let body = futures_util::stream::iter(std::iter::once(header_line).chain(std::iter::repeat_n(chunk, 512)).map(Ok::<Bytes, Infallible>));
        let mut resp = Response::new(Body::from_stream(body));
        resp.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/csv"));
        resp
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, Router::new().fallback(stream)).await;
    });
    format!("http://{addr}/big.csv")
}

async fn too_large(client: &McpClient, tool: &str, args: Value) -> Value {
    let err = client.tools_call(tool, args).await.expect_err("import_too_large");
    assert_eq!(err.error_code.as_deref(), Some("import_too_large"), "{err:?}");
    err.data
}

#[tokio::test]
async fn tblurl_ac03_a_declared_oversize_body_is_refused_with_the_cap() {
    let server = capped_server().await;
    let (ns, key) = signup(&server.base_url, "TBLURL AC3").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let fixture = Fixture::start(&orders_csv(10_000), false).await;
    assert!(orders_csv(10_000).len() as i64 > CAP);

    client.tools_call("host.table.create", json!({"name": "t", "columns": {"id": "integer", "name": "text", "qty": "integer"}})).await.expect("create");
    client.tools_call("host.table.append", json!({"table": "t", "rows": [{"id": 1, "name": "a", "qty": 1}]})).await.expect("seed row");
    let data = too_large(&client, "host.table.append", json!({"table": "t", "source_url": fixture.csv_url()})).await;
    assert_eq!(data["bytes"], json!(CAP), "{data}");
    let count = client.tools_call("host.table.count", json!({"table": "t"})).await.expect("count");
    assert_eq!(extract_structured(&count)["count"], json!(1), "the table is unchanged");

    let data = too_large(&client, "host.table.create", json!({"name": "fresh", "source_url": fixture.csv_url()})).await;
    assert_eq!(data["bytes"], json!(CAP), "{data}");
    let tables = client.tools_call("host.table.list", json!({})).await.expect("list");
    let names = extract_structured(&tables).to_string();
    assert!(!names.contains("fresh"), "no table was created: {names}");

    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.expect("lookup").expect("tenant");
    let rows = ledger(&server.state, &tenant).await.expect("ledger");
    assert!(rows.iter().all(|r| r["outcome"] == json!("import_too_large") && r["bytes"] == json!(0)), "{rows:?}");
}

#[tokio::test]
async fn tblurl_ac03_a_streamed_body_is_cut_off_one_chunk_past_the_cap() {
    let server = capped_server().await;
    let (ns, key) = signup(&server.base_url, "TBLURL AC3 stream").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let url = streaming_csv().await;

    let data = too_large(&client, "host.table.create", json!({"name": "streamed", "source_url": url})).await;
    assert_eq!(data["bytes"], json!(CAP), "{data}");
    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.expect("lookup").expect("tenant");
    let rows = ledger(&server.state, &tenant).await.expect("ledger");
    let received = rows[0]["bytes"].as_i64().expect("bytes");
    assert!(received > CAP && received < 1024 * 1024, "stopped soon after the cap, not after 4 MB: {received}");
    let tables = client.tools_call("host.table.list", json!({})).await.expect("list");
    assert!(!extract_structured(&tables).to_string().contains("streamed"));
}
