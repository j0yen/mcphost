//! PRD-mcphost-blob-virus-and-type-guard AC4 — with the clamd socket pointing
//! at the fake clamd, EICAR is 451 `malware_found {signature:
//! "Eicar-Test-Signature"}`; with a socket path that does not exist the
//! answer is 503 `scan_unavailable` (fail closed) and nothing is stored.
use crate::blobguard;

use mcphost::blobs::{upload, MemorySink, Policy, UploadRequest};

#[test]
fn eicar_is_451_and_missing_socket_is_503_and_clean_passes_clamav_mode() {
    let dir = blobguard::fresh_dir("ac04");
    let blobs = dir.join("blobs");
    let sock = blobguard::fake_clamd(&dir);
    let sink = MemorySink::default();
    let policy = Policy::default();

    let r = upload(&blobs, &UploadRequest { link_token_id: "l", filename: "eicar.txt", bytes: blobguard::EICAR }, &policy, Some(&sock), &sink);
    assert_eq!(r.status, 451);
    assert_eq!(r.body["error"], "malware_found");
    assert_eq!(r.body["signature"], "Eicar-Test-Signature");

    let gone = dir.join("no-such.ctl");
    let r = upload(&blobs, &UploadRequest { link_token_id: "l", filename: "ok.txt", bytes: b"hello" }, &policy, Some(&gone), &sink);
    assert_eq!((r.status, r.body["error"].as_str()), (503, Some("scan_unavailable")));
    assert!(!blobs.exists() || mcphost::blobs::stored_files(&blobs).is_empty(), "file not stored");

    let msgs = sink.0.lock().unwrap();
    assert_eq!(msgs[0]["signature"], "Eicar-Test-Signature");
    assert_eq!(msgs[1]["reason"], "scan_unavailable");
    drop(msgs);

    // Control: with a live clamd a clean file is stored, scan mode clamav.
    let r = upload(&blobs, &UploadRequest { link_token_id: "l", filename: "ok.txt", bytes: b"hello" }, &policy, Some(&sock), &sink);
    assert_eq!(r.status, 200);
    assert_eq!(r.body["scan"]["mode"], "clamav");
}
