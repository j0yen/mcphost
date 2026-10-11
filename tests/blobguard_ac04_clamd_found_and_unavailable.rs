//! PRD-mcphost-blob-virus-and-type-guard AC4 — with `MCPHOST_CLAMD_SOCKET`
//! pointing at the fake clamd, EICAR is 451 `malware_found {signature:
//! "Eicar-Test-Signature"}`; with a socket path that does not exist the
//! answer is 503 `scan_unavailable` (fail closed) and nothing is stored.
//!
//! The parent runs the fake clamd and re-executes this binary with the env
//! var set; the child goes through `upload_configured`, the `/up/{token}`
//! entry point, which reads the socket from the environment itself.
use crate::blobguard;

use mcphost::blobs::{upload_configured, MemorySink, Policy, UploadRequest, CLAMD_SOCKET_ENV};

const CHILD: &str = "blobguard_ac04_clamd_found_and_unavailable::child_uploads_through_env_configured_scanner";

#[test]
fn eicar_is_451_and_missing_socket_is_503_via_env_var() {
    let dir = blobguard::fresh_dir("ac04");
    let sock = blobguard::fake_clamd(&dir);

    let live = dir.join("live");
    std::fs::create_dir_all(&live).unwrap();
    let out = blobguard::run_child(CHILD, &[(CLAMD_SOCKET_ENV, Some(sock.as_os_str())), (blobguard::CHILD_DIR_ENV, Some(live.as_os_str()))]);
    assert!(out.contains("mode=live"), "{out}");

    let gone = dir.join("gone");
    std::fs::create_dir_all(&gone).unwrap();
    let missing = dir.join("no-such.ctl");
    let out = blobguard::run_child(CHILD, &[(CLAMD_SOCKET_ENV, Some(missing.as_os_str())), (blobguard::CHILD_DIR_ENV, Some(gone.as_os_str()))]);
    assert!(out.contains("mode=missing"), "{out}");
    assert!(mcphost::blobs::stored_files(&gone.join("blobs")).is_empty(), "nothing stored when the scanner is down");
}

#[test]
fn child_uploads_through_env_configured_scanner() {
    let Some(dir) = std::env::var_os(blobguard::CHILD_DIR_ENV).map(std::path::PathBuf::from) else { return };
    let sock = std::path::PathBuf::from(std::env::var_os(CLAMD_SOCKET_ENV).expect("parent sets the socket"));
    let blobs = dir.join("blobs");
    let sink = MemorySink::default();
    let policy = Policy::default();
    let post = |name: &str, bytes: &[u8]| upload_configured(&blobs, &UploadRequest { link_token_id: "l", filename: name, bytes }, &policy, &sink);

    if sock.exists() {
        let r = post("eicar.txt", blobguard::EICAR);
        assert_eq!((r.status, r.body["error"].as_str()), (451, Some("malware_found")));
        assert_eq!(r.body["signature"], "Eicar-Test-Signature");
        assert_eq!(sink.0.lock().unwrap()[0]["signature"], "Eicar-Test-Signature");
        // Control: a clean file is stored, scanned by clamd.
        let r = post("ok.txt", b"hello");
        assert_eq!(r.status, 200);
        assert_eq!(r.body["scan"]["mode"], "clamav");
        println!("mode=live");
    } else {
        let r = post("ok.txt", b"hello");
        assert_eq!((r.status, r.body["error"].as_str()), (503, Some("scan_unavailable")));
        assert_eq!(sink.0.lock().unwrap()[0]["reason"], "scan_unavailable");
        assert!(mcphost::blobs::stored_files(&blobs).is_empty(), "file not stored");
        println!("mode=missing");
    }
}
