//! PRD-mcphost-blob-virus-and-type-guard AC2 — a link with
//! `accept: ["text/csv"]` refuses `report.exe` (PE magic) as
//! `type_not_accepted` and `data.csv` holding PE bytes as `magic_mismatch`,
//! both 415; nothing lands under `blobs/` and the inbox holds two
//! `blob.rejected` messages naming the reasons.
use crate::blobguard;

use mcphost::blobs::{upload, MemorySink, Policy, UploadRequest};

#[test]
fn exe_and_pe_as_csv_are_refused_415_with_reasons_and_nothing_stored() {
    let dir = blobguard::fresh_dir("ac02");
    let blobs = dir.join("blobs");
    let sink = MemorySink::default();
    let policy = Policy { accept: Some(vec!["text/csv".into()]), ..Policy::default() };

    let r1 = upload(&blobs, &UploadRequest { link_token_id: "lnk_1", filename: "report.exe", bytes: blobguard::PE }, &policy, None, &sink);
    let r2 = upload(&blobs, &UploadRequest { link_token_id: "lnk_1", filename: "data.csv", bytes: blobguard::PE }, &policy, None, &sink);
    assert_eq!((r1.status, r1.body["error"].as_str()), (415, Some("type_not_accepted")));
    assert_eq!((r2.status, r2.body["error"].as_str()), (415, Some("magic_mismatch")));

    assert!(!blobs.exists() || mcphost::blobs::stored_files(&blobs).is_empty(), "nothing under blobs/");
    let inbox = sink.0.lock().unwrap();
    assert_eq!(inbox.len(), 2);
    assert!(inbox.iter().all(|m| m["type"] == "blob.rejected" && m["link_token_id"] == "lnk_1"));
    assert_eq!(inbox[0]["reason"], "type_not_accepted");
    assert_eq!(inbox[0]["name"], "report.exe");
    assert_eq!(inbox[1]["reason"], "magic_mismatch");
    assert_eq!(inbox[1]["bytes"], blobguard::PE.len());
    drop(inbox);

    // Control: a real CSV passes the same link and IS stored.
    let ok = upload(&blobs, &UploadRequest { link_token_id: "lnk_1", filename: "ok.csv", bytes: b"a,b\n1,2\n" }, &policy, None, &sink);
    assert_eq!(ok.status, 200);
    assert_eq!(sink.0.lock().unwrap().len(), 2, "no rejection for the good file");
}
