//! PRD-mcphost-blob-put-get-signed-url
//! AC4 — Given a free tenant at `blobs_max` or whose next put would exceed
//! `blob_bytes_max` or `blob_max_bytes`, When `host.blob.put` runs, Then the
//! refusal code is `quota_blobs`, `quota_blob_bytes` or `blob_too_large`
//! with `{limit, used}`, and `host.quickstart.limits.plan` names all three
//! ceilings with the same numbers.
//!
//! The 1 MiB `/mcp` request-body cap means a 20 MiB put cannot arrive over
//! HTTP, so the put-side cases call [`mcphost::blobs::blob_put`] directly on
//! a [`common::bare_state`], with the free plan's real (unmodified) numbers.

use crate::common;
use base64::Engine as _;
use common::{McpClient, TestServer, signup};
use mcphost::blobs;
use mcphost::plans::PlanCatalog;
use serde_json::{Value, json};

const MIB: i64 = 1024 * 1024;

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-blobput-ac04-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn put_args(name: &str, fill: u8, len: usize) -> Value {
    json!({
        "name": name,
        "mime": "application/octet-stream",
        "content_base64": base64::engine::general_purpose::STANDARD.encode(vec![fill; len]),
    })
}

fn free_ceilings() -> (i64, i64, i64) {
    let catalog = PlanCatalog::default_catalog();
    let free = catalog.get("free").expect("free plan");
    (free.blobs_max, free.blob_bytes_max, free.blob_max_bytes)
}

#[test]
fn free_plan_ceilings_are_the_prd_numbers() {
    assert_eq!(free_ceilings(), (100, 50 * MIB, 20 * MIB));
    let catalog = PlanCatalog::default_catalog();
    let pro = catalog.get("pro").expect("pro plan");
    assert_eq!((pro.blobs_max, pro.blob_bytes_max, pro.blob_max_bytes), (10_000, 2048 * MIB, 200 * MIB));
}

#[tokio::test]
async fn put_over_blob_max_bytes_is_refused_with_limit_and_used() {
    let dir = scratch_dir("too-large");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "blobput-ac04-large").await;
    let (_, _, max_bytes) = free_ceilings();

    let over = (max_bytes + 1) as usize;
    let err = blobs::blob_put(&state, &tenant, &put_args("big.bin", 7, over))
        .await
        .expect_err("a file over blob_max_bytes must be refused");
    assert_eq!(err.code(), "blob_too_large", "unexpected error: {err:?}");
    let msg = format!("{err:?}");
    assert!(msg.contains(&max_bytes.to_string()) && msg.contains(&over.to_string()), "{msg}");
    assert_eq!(state.db.blobs_usage(tenant.id).await.unwrap(), (0, 0), "nothing stored");

    blobs::blob_put(&state, &tenant, &put_args("fits.bin", 7, max_bytes as usize))
        .await
        .expect("exactly blob_max_bytes is allowed");
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn put_that_passes_blob_bytes_max_is_refused() {
    let dir = scratch_dir("bytes");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "blobput-ac04-bytes").await;
    let (_, total_max, file_max) = free_ceilings();

    // Two 20 MiB files, then 10 MiB more reaches the 50 MiB ceiling exactly;
    // one more byte passes it.
    for (i, name) in ["a.bin", "b.bin"].iter().enumerate() {
        blobs::blob_put(&state, &tenant, &put_args(name, i as u8 + 1, file_max as usize))
            .await
            .expect("within the ceiling");
    }
    let used = 2 * file_max;
    blobs::blob_put(&state, &tenant, &put_args("c.bin", 3, (total_max - used) as usize))
        .await
        .expect("filling the ceiling exactly is allowed");
    let err = blobs::blob_put(&state, &tenant, &put_args("d.bin", 4, 1))
        .await
        .expect_err("one byte past blob_bytes_max must be refused");
    assert_eq!(err.code(), "quota_blob_bytes", "unexpected error: {err:?}");
    let msg = format!("{err:?}");
    assert!(msg.contains(&total_max.to_string()), "{msg}");

    // Replacing a blob counts only the growth: swapping a.bin for smaller
    // bytes is fine even at the ceiling.
    blobs::blob_put(&state, &tenant, &put_args("a.bin", 9, 10))
        .await
        .expect("a smaller replacement frees room");
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn put_of_a_new_name_at_blobs_max_is_refused() {
    let dir = scratch_dir("count");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "blobput-ac04-count").await;
    let (blobs_max, _, _) = free_ceilings();

    for i in 0..blobs_max {
        blobs::blob_put(&state, &tenant, &put_args(&format!("f{i}.bin"), i as u8, 4))
            .await
            .expect("under blobs_max");
    }
    let err = blobs::blob_put(&state, &tenant, &put_args("one-more.bin", 1, 4))
        .await
        .expect_err("a new name past blobs_max must be refused");
    assert_eq!(err.code(), "quota_blobs", "unexpected error: {err:?}");
    let msg = format!("{err:?}");
    assert!(msg.contains(&blobs_max.to_string()), "{msg}");

    // A new version of an existing name is not a new blob.
    blobs::blob_put(&state, &tenant, &put_args("f0.bin", 99, 8))
        .await
        .expect("re-putting an existing name at blobs_max is allowed");
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn quickstart_limits_plan_names_all_three_ceilings_with_the_same_numbers() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Blobput AC4 Quickstart").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let structured = common::extract_structured(
        &client.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("host.quickstart"),
    );
    let plan = &structured["limits"]["plan"];
    let (blobs_max, blob_bytes_max, blob_max_bytes) = free_ceilings();
    assert_eq!(plan["blobs_max"], json!(blobs_max));
    assert_eq!(plan["blob_bytes_max"], json!(blob_bytes_max));
    assert_eq!(plan["blob_max_bytes"], json!(blob_max_bytes));
}
