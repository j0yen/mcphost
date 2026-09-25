//! PRD-mcphost-document-store
//! AC8 -- Given a document with unsupported mime (`image/png` by content
//! sniff), When `put` runs, Then it is rejected naming the allowed mimes.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn put_rejects_a_png_sniffed_from_content_naming_allowed_mimes() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Docs AC8 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // A real PNG magic-number header (no `mime` argument given, so this
    // must be caught by content sniffing, not name-extension inference).
    use base64::Engine as _;
    let png_bytes: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDRnotrealpngdata";
    let content_base64 = base64::engine::general_purpose::STANDARD.encode(png_bytes);

    let err = client
        .tools_call(
            "host.docs.put",
            json!({"name": "photo.png", "content_base64": content_base64}),
        )
        .await
        .expect_err("a sniffed-png document must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("docs_mime_unsupported"), "error: {err:?}");
    assert_eq!(err.data["mime"], json!("image/png"), "error data: {:?}", err.data);
    let allowed = err.data["allowed"].as_array().expect("allowed array");
    for want in ["text/plain", "text/markdown", "application/json", "text/csv"] {
        assert!(
            allowed.iter().any(|v| v == want),
            "allowed mimes must name {want}: {allowed:?}"
        );
    }
}
