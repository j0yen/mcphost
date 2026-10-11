//! PRD-mcphost-blob-virus-and-type-guard AC7 — an SVG with a `<script>`
//! element POSTed to a default link is 415 `active_content`; with
//! `allow_active: true` it is stored and served with
//! `Content-Disposition: attachment`.
use crate::blobguard;

use mcphost::blobs::{serve_headers, upload, MemorySink, Policy, UploadRequest};

const SVG: &[u8] = b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>";

#[test]
fn svg_script_refused_by_default_and_stored_as_attachment_when_allowed() {
    let dir = blobguard::fresh_dir("ac07");
    let blobs = dir.join("blobs");
    let sink = MemorySink::default();

    let r = upload(&blobs, &UploadRequest { link_token_id: "l", filename: "x.svg", bytes: SVG }, &Policy::default(), None, &sink);
    assert_eq!((r.status, r.body["error"].as_str()), (415, Some("active_content")));
    assert!(!blobs.exists() || mcphost::blobs::stored_files(&blobs).is_empty());
    assert_eq!(sink.0.lock().unwrap()[0]["reason"], "active_content");

    let allowed = Policy { allow_active: true, ..Policy::default() };
    let r = upload(&blobs, &UploadRequest { link_token_id: "l", filename: "x.svg", bytes: SVG }, &allowed, None, &sink);
    assert_eq!(r.status, 200, "{:?}", r.body);
    let id = r.body["blob_id"].as_str().unwrap();
    let headers = serve_headers(&blobs, id).unwrap();
    assert!(headers.contains(&("Content-Disposition".to_string(), "attachment".to_string())), "{headers:?}");
}
