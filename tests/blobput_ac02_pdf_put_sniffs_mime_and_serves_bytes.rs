//! PRD-mcphost-blob-put-get-signed-url
//! AC2 — Given a 300 KB PDF as `content_base64`, When
//! `host.blob.put {name: "report.pdf"}` runs, Then the response carries
//! `mime: application/pdf` (sniffed), `sha256` equal to the fixture's, and a
//! `url` that answers HTTP 200 with `Content-Type: application/pdf` and the
//! same bytes.

use crate::common;
use base64::Engine as _;
use common::{McpClient, TestServer, extract_structured, signup};

pub fn fixture_pdf() -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/blobput/report.pdf");
    std::fs::read(path).expect("read report.pdf fixture")
}

#[tokio::test]
async fn pdf_put_sniffs_mime_and_url_serves_the_same_bytes() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Blobput AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let pdf = fixture_pdf();
    assert_eq!(pdf.len(), 300 * 1024);

    let result = client
        .tools_call(
            "host.blob.put",
            serde_json::json!({
                "name": "report.pdf",
                "content_base64": base64::engine::general_purpose::STANDARD.encode(&pdf),
            }),
        )
        .await
        .expect("host.blob.put");
    let put = extract_structured(&result);

    assert_eq!(put["mime"], "application/pdf");
    assert_eq!(put["sha256"], mcphost::billing::sha256_hex(&pdf));
    assert_eq!(put["bytes"], pdf.len());
    assert_eq!(put["name"], "report.pdf");
    assert_eq!(put["version"], 1);
    assert!(put["id"].as_str().is_some_and(|s| !s.is_empty()));
    assert!(put["expires"].as_i64().is_some_and(|e| e > mcphost::state::now_unix()));

    let url = put["url"].as_str().expect("url").to_string();
    let resp = reqwest::get(&url).await.expect("GET blob url");
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "url: {url}");
    assert_eq!(
        resp.headers().get("content-type").and_then(|v| v.to_str().ok()),
        Some("application/pdf")
    );
    let body = resp.bytes().await.expect("body");
    assert_eq!(body.as_ref(), pdf.as_slice(), "the url serves the stored bytes");
}

#[tokio::test]
async fn same_name_new_bytes_is_a_new_version_and_identical_bytes_a_no_op() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Blobput AC2 Versions").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);

    let put = |bytes: Vec<u8>| {
        let client = &client;
        async move {
            extract_structured(
                &client
                    .tools_call(
                        "host.blob.put",
                        serde_json::json!({"name": "notes.txt", "content_base64": b64(&bytes)}),
                    )
                    .await
                    .expect("host.blob.put"),
            )
        }
    };
    let v1 = put(b"first".to_vec()).await;
    let again = put(b"first".to_vec()).await;
    let v2 = put(b"second".to_vec()).await;

    assert_eq!(v1["mime"], "text/plain");
    assert_eq!(again["id"], v1["id"]);
    assert_eq!(again["version"], 1, "identical sha256 is a no-op");
    assert_eq!(v2["id"], v1["id"], "same name keeps the id");
    assert_eq!(v2["version"], 2);
    let body = reqwest::get(v2["url"].as_str().unwrap()).await.unwrap().text().await.unwrap();
    assert_eq!(body, "second");
}
