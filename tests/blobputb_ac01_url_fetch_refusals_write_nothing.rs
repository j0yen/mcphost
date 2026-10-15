//! PRD-mcphost-blob-put-get-signed-url-b
//! AC1 — Given `host.blob.put {name, url: "http://169.254.169.254/x"}` or a
//! 25 MB remote file on the free plan, When it runs, Then the refusal is
//! `host_not_allowed` or `blob_too_large` and nothing is written under
//! `blobs/`.

use crate::common;
use mcphost::blobs;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-blobputb-ac05-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// No file at all under `<data_dir>/blobs/`.
fn nothing_written(dir: &std::path::Path) -> bool {
    fn count(p: &std::path::Path) -> usize {
        match std::fs::read_dir(p) {
            Err(_) => 0,
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|e| if e.path().is_dir() { count(&e.path()) } else { 1 })
                .sum(),
        }
    }
    count(&dir.join("blobs")) == 0
}

#[tokio::test]
async fn link_local_metadata_address_is_host_not_allowed() {
    let dir = scratch_dir("ssrf");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "blobput-ac05-ssrf").await;

    for url in [
        "http://169.254.169.254/x",
        "http://10.0.0.5/x",
        "http://192.168.1.1/x",
        "http://[fe80::1]/x",
        "http://metadata.internal/x",
    ] {
        let err = blobs::blob_put(&state, &tenant, &json!({"name": "m.bin", "url": url}))
            .await
            .expect_err("a private target must be refused");
        assert_eq!(err.code(), "host_not_allowed", "{url}: {err:?}");
    }
    assert!(nothing_written(&dir), "nothing under blobs/");
    assert_eq!(state.db.blobs_usage(tenant.id).await.unwrap(), (0, 0));
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_25_mb_remote_file_is_blob_too_large_and_writes_nothing() {
    let dir = scratch_dir("big");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "blobput-ac05-big").await;
    let stub = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/big.bin"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0u8; 25_000_000]))
        .mount(&stub)
        .await;

    let url = format!("{}/big.bin", stub.uri());
    let err = blobs::blob_put(&state, &tenant, &json!({"name": "big.bin", "url": url}))
        .await
        .expect_err("25 MB is over the free plan's 20 MiB");
    assert_eq!(err.code(), "blob_too_large", "unexpected error: {err:?}");
    assert!(nothing_written(&dir), "nothing under blobs/");
    assert_eq!(state.db.blobs_usage(tenant.id).await.unwrap(), (0, 0));
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_small_remote_file_is_stored_and_typed_by_its_bytes() {
    let dir = scratch_dir("ok");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "blobput-ac05-ok").await;
    let stub = MockServer::start().await;
    let png: Vec<u8> = b"\x89PNG\r\n\x1a\n0000".to_vec();
    Mock::given(method("GET"))
        .and(path("/logo"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(png.clone()))
        .mount(&stub)
        .await;

    let out = blobs::blob_put(&state, &tenant, &json!({"name": "logo.png", "url": format!("{}/logo", stub.uri())}))
        .await
        .expect("a small public-enough file is fetched");
    assert_eq!(out["mime"], "image/png");
    assert_eq!(out["bytes"], png.len());
    assert!(!nothing_written(&dir));

    let err = blobs::blob_put(&state, &tenant, &json!({"name": "x", "url": "http://a/", "content_base64": "AA=="}))
        .await
        .expect_err("url and content_base64 together are ambiguous");
    assert_eq!(err.code(), "args_invalid");
    std::fs::remove_dir_all(&dir).ok();
}
