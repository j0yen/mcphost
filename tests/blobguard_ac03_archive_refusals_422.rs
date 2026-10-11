//! PRD-mcphost-blob-virus-and-type-guard AC3 — under `accept:
//! ["application/zip"]`: a ~1 KB zip declaring 10 GB, a zip containing
//! `../etc/passwd`, and a zip containing a zip are refused 422 as
//! `archive_ratio`, `archive_path`, `archive_nested`; a zip of 40 CSVs is
//! accepted with `scan.entries: 40`.
use crate::blobguard;

use mcphost::blobs::{upload, MemorySink, Policy, UploadRequest};

fn post(name: &str, bytes: &[u8], sink: &MemorySink, dir: &std::path::Path) -> mcphost::blobs::UploadResponse {
    let policy = Policy { accept: Some(vec!["application/zip".into()]), zip_entries_accept: Some(vec!["text/csv".into()]), ..Policy::default() };
    upload(dir, &UploadRequest { link_token_id: "lnk_z", filename: name, bytes }, &policy, None, sink)
}

#[test]
fn bomb_traversal_nested_refused_422_and_forty_csvs_accepted() {
    let dir = blobguard::fresh_dir("ac03");
    let blobs = dir.join("blobs");
    let sink = MemorySink::default();

    // 10 GB does not fit a u32 central-directory field; 0xFFFFFFFF (zip64 marker, >= 4 GiB) is what a bomb declares.
    let bomb = blobguard::zip(&[("big.csv", &[b'a'; 900][..], Some(u32::MAX))]);
    assert!(bomb.len() < 1500, "bomb is about 1 KB, got {}", bomb.len());
    let traversal = blobguard::zip(&[("../etc/passwd", b"root", None)]);
    let inner = blobguard::zip(&[("a.csv", b"x\n", None)]);
    let nested = blobguard::zip(&[("inner.zip", &inner, None)]);

    for (name, bytes, reason) in [("bomb.zip", &bomb, "archive_ratio"), ("t.zip", &traversal, "archive_path"), ("n.zip", &nested, "archive_nested")] {
        let r = post(name, bytes, &sink, &blobs);
        assert_eq!((r.status, r.body["error"].as_str()), (422, Some(reason)), "{name}");
    }
    assert!(!blobs.exists() || mcphost::blobs::stored_files(&blobs).is_empty());

    let names: Vec<String> = (0..40).map(|i| format!("p{i:02}.csv")).collect();
    let entries: Vec<blobguard::ZipSpec<'_>> = names.iter().map(|n| (n.as_str(), &b"a,b\n1,2\n"[..], None)).collect();
    let good = blobguard::zip(&entries);
    let r = post("forty.zip", &good, &sink, &blobs);
    assert_eq!(r.status, 200, "{:?}", r.body);
    assert_eq!(r.body["scan"]["entries"], 40);

    let wrong = blobguard::zip(&[("run.exe", b"MZ", None)]);
    assert_eq!(post("w.zip", &wrong, &sink, &blobs).body["error"], "archive_entry_type");
}
