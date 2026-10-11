//! PRD-mcphost-blob-virus-and-type-guard AC5 — with `MCPHOST_CLAMD_SOCKET`
//! unset a PNG is stored with `scan: {mode: "allowlist", result: "pass"}`
//! and the guard adds under 50 ms for a 20 MB file.
//!
//! The parent re-executes this binary with the env var removed; the child
//! goes through `upload_configured`, which reads the environment itself.
use crate::blobguard;

use mcphost::blobs::{upload_configured, MemorySink, Policy, UploadRequest, CLAMD_SOCKET_ENV};

const CHILD: &str = "blobguard_ac05_allowlist_mode_png_fast::child_stores_png_in_allowlist_mode";

#[test]
fn png_stored_with_allowlist_scan_and_guard_under_50ms_for_20mb() {
    let dir = blobguard::fresh_dir("ac05");
    let out = blobguard::run_child(CHILD, &[(CLAMD_SOCKET_ENV, None), (blobguard::CHILD_DIR_ENV, Some(dir.as_os_str()))]);
    assert!(out.contains("stored=allowlist"), "{out}");
}

#[test]
fn child_stores_png_in_allowlist_mode() {
    let Some(dir) = std::env::var_os(blobguard::CHILD_DIR_ENV).map(std::path::PathBuf::from) else { return };
    assert!(std::env::var_os(CLAMD_SOCKET_ENV).is_none(), "parent removed the socket env");
    let blobs = dir.join("blobs");
    let sink = MemorySink::default();
    let png = blobguard::png_bytes(20 * 1024 * 1024);
    let policy = Policy { blob_max_bytes: 64 * 1024 * 1024, ..Policy::default() };
    let post = |dest: &std::path::Path| upload_configured(dest, &UploadRequest { link_token_id: "l", filename: "big.png", bytes: &png }, &policy, &sink);

    // Time added by the guard = the guarded upload minus a bare store of
    // the same bytes.
    let t = std::time::Instant::now();
    let r = post(&blobs);
    let guarded = t.elapsed();
    let scan = mcphost::blobs::Scan { mode: "allowlist", mime: "image/png".into(), entries: None };
    let t = std::time::Instant::now();
    mcphost::blobs::store(&dir.join("bare"), "big.png", &png, &scan).unwrap();
    let bare = t.elapsed();
    let added = guarded.saturating_sub(bare);
    println!("guarded={guarded:?} bare_store={bare:?} added={added:?}");

    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body["scan"]["mode"], "allowlist");
    assert_eq!(r.body["scan"]["result"], "pass");
    let id = r.body["blob_id"].as_str().unwrap();
    let row: serde_json::Value = serde_json::from_slice(&std::fs::read(blobs.join(format!("{id}.json"))).unwrap()).unwrap();
    assert_eq!(row["scan_json"]["mode"], "allowlist");
    assert_eq!(row["scan_json"]["result"], "pass");
    assert!(added < std::time::Duration::from_millis(50), "guard added {added:?}");
    println!("stored=allowlist");
}
