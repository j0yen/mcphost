//! PRD-mcphost-blob-virus-and-type-guard AC5 — with no clamd socket
//! configured a PNG is stored with `scan: {mode: "allowlist", result:
//! "pass"}` and the guard adds under 50 ms for a 20 MB file.
use crate::blobguard;

use mcphost::blobs::{check, upload, MemorySink, Policy, UploadRequest};

#[test]
fn png_stored_with_allowlist_scan_and_guard_under_50ms_for_20mb() {
    let dir = blobguard::fresh_dir("ac05");
    let blobs = dir.join("blobs");
    let sink = MemorySink::default();
    let png = blobguard::png_bytes(20 * 1024 * 1024);
    let policy = Policy { blob_max_bytes: 64 * 1024 * 1024, ..Policy::default() };

    let t = std::time::Instant::now();
    let scan = check("big.png", &png, &policy, None).expect("png passes");
    let took = t.elapsed();
    println!("guard time for 20 MB: {took:?}");
    assert_eq!(scan.mode, "allowlist");
    assert!(took < std::time::Duration::from_millis(50), "guard took {took:?}");

    let r = upload(&blobs, &UploadRequest { link_token_id: "l", filename: "big.png", bytes: &png }, &policy, None, &sink);
    assert_eq!(r.status, 200);
    assert_eq!(r.body["scan"]["mode"], "allowlist");
    assert_eq!(r.body["scan"]["result"], "pass");
    let id = r.body["blob_id"].as_str().unwrap();
    let row: serde_json::Value = serde_json::from_slice(&std::fs::read(blobs.join(format!("{id}.json"))).unwrap()).unwrap();
    assert_eq!(row["scan_json"]["mode"], "allowlist");
    assert_eq!(row["scan_json"]["result"], "pass");
}
