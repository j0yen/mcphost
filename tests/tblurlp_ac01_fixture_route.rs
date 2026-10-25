//! PRD-mcphost-table-import-url-live-proof
//! AC1 (P0) -- Given the binary's static routes, When
//! `GET /examples/sample-1000.csv` is requested, Then it answers 200 with
//! `Content-Type: text/csv; charset=utf-8`, the same `Cache-Control` as
//! `/llms.txt`, and a body byte-identical to
//! `examples/database-in-a-minute/fixture.csv` (header plus 1,000 data rows).

use crate::common::TestServer;

fn cache_control(response: &reqwest::Response) -> Option<String> {
    response.headers().get("cache-control").and_then(|v| v.to_str().ok()).map(str::to_string)
}

#[tokio::test]
async fn tblurlp_ac01_fixture_route_serves_the_committed_csv() {
    let server = TestServer::start().await;
    let client = reqwest::Client::new();
    let response = client
        .get(format!("{}/examples/sample-1000.csv", server.base_url))
        .send()
        .await
        .expect("GET /examples/sample-1000.csv");

    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers().get("content-type").and_then(|v| v.to_str().ok()),
        Some("text/csv; charset=utf-8")
    );

    let llms = client.get(format!("{}/llms.txt", server.base_url)).send().await.expect("GET /llms.txt");
    assert_eq!(llms.status(), 200);
    assert_eq!(cache_control(&response), cache_control(&llms), "same Cache-Control as /llms.txt");

    let body = response.bytes().await.expect("body");
    let fixture = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/database-in-a-minute/fixture.csv"))
        .expect("read fixture.csv");
    assert_eq!(body.as_ref(), fixture.as_slice(), "byte-identical to the committed fixture");
    let text = String::from_utf8(fixture).expect("utf-8 fixture");
    assert_eq!(text.lines().count(), 1001, "header plus 1,000 data rows");
    assert_eq!(text.lines().next(), Some("id,category,amount,day"));
}
