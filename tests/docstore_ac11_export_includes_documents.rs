//! PRD-mcphost-document-store
//! AC11 -- Given a tenant export, When `host.export` runs, Then the bundle
//! includes the tenant's documents with their current version text.

use std::collections::HashMap;
use std::io::Read;
use std::time::{Duration, Instant};

use flate2::read::GzDecoder;
use tar::Archive;

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

fn read_archive_entries(path: &std::path::Path) -> HashMap<String, Vec<u8>> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read archive {path:?}: {e}"));
    let decoder = GzDecoder::new(&bytes[..]);
    let mut archive = Archive::new(decoder);
    let mut out = HashMap::new();
    for entry in archive.entries().expect("tar entries") {
        let mut entry = entry.expect("tar entry");
        let entry_path = entry.path().expect("entry path").to_string_lossy().to_string();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).expect("read entry data");
        out.insert(entry_path, data);
    }
    out
}

async fn wait_for_done(client: &McpClient, run_id: &str) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("host.runs.get ok"),
        );
        if got["status"] == json!("done") || got["status"] == json!("error") {
            return got;
        }
        assert!(Instant::now() < deadline, "export never finished: {got:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn export_bundle_includes_documents_with_current_version_text() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Export AC11 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let put = extract_structured(
        &client
            .tools_call("host.docs.put", json!({"name": "handbook.md", "content": "# hi"}))
            .await
            .expect("docs put ok"),
    );
    let doc_id = put["id"].as_str().expect("id").to_string();
    // Put a second version -- the export must carry the CURRENT text, not
    // the first one.
    client
        .tools_call("host.docs.put", json!({"name": "handbook.md", "content": "# hi v2"}))
        .await
        .expect("docs put v2 ok");

    let enqueue = extract_structured(
        &client
            .tools_call("host.export", json!({}))
            .await
            .expect("host.export ok"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id field").to_string();
    let finished = wait_for_done(&client, &run_id).await;
    assert_eq!(finished["status"], json!("done"), "export failed: {finished:?}");

    let archive_path = server.state.db.data_dir().join("exports").join(format!("{run_id}.tar.gz"));
    let entries = read_archive_entries(&archive_path);

    let entry_name = format!("documents/{doc_id}.json");
    let doc_bytes = entries
        .get(&entry_name)
        .unwrap_or_else(|| panic!("archive must contain {entry_name}: {:?}", entries.keys()));
    let doc_json: serde_json::Value = serde_json::from_slice(doc_bytes).expect("documents/<id>.json is valid JSON");
    assert_eq!(doc_json["id"], json!(doc_id));
    assert_eq!(doc_json["name"], json!("handbook.md"));
    assert_eq!(doc_json["version"], json!(2));
    assert_eq!(doc_json["text"], json!("# hi v2"), "must carry the current version's text: {doc_json:?}");
}
