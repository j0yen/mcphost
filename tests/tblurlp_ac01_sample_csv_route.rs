//! PRD-mcphost-table-import-url-live-proof
//! AC1 (P0) -- Given the binary's static routes, When
//! `GET /examples/sample-1000.csv` is requested, Then it answers 200 with
//! `Content-Type: text/csv; charset=utf-8`, the same `Cache-Control` as
//! `/llms.txt`, and a body byte-identical to
//! `examples/database-in-a-minute/fixture.csv` (header plus 1,000 data rows).

use crate::common;
use common::TestServer;

#[tokio::test]
async fn tblurlp_ac01_sample_csv_is_served_byte_identical_with_llms_txt_cache_control() {
    let server = TestServer::start().await;
    let client = reqwest::Client::new();
    let csv = client
        .get(format!("{}/examples/sample-1000.csv", server.base_url))
        .send()
        .await
        .expect("GET /examples/sample-1000.csv");
    let llms = client
        .get(format!("{}/llms.txt", server.base_url))
        .send()
        .await
        .expect("GET /llms.txt");

    assert_eq!(csv.status(), 200);
    let header = |r: &reqwest::Response, name: &str| {
        r.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string)
    };
    assert_eq!(header(&csv, "content-type").as_deref(), Some("text/csv; charset=utf-8"));
    assert_eq!(
        header(&csv, "cache-control"),
        header(&llms, "cache-control"),
        "same Cache-Control as /llms.txt"
    );

    let body = csv.bytes().await.expect("body");
    let fixture = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/examples/database-in-a-minute/fixture.csv"
    ))
    .expect("read fixture.csv");
    assert_eq!(body.as_ref(), fixture.as_slice(), "served bytes must equal the committed fixture");
    let text = String::from_utf8(fixture).expect("utf-8 fixture");
    assert_eq!(text.lines().count(), 1001, "header plus 1,000 data rows");
    assert_eq!(text.lines().next(), Some("id,category,amount,day"));
}
