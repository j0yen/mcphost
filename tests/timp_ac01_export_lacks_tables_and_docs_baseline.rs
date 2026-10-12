//! PRD-mcphost-export-tables-and-import-restore
//! AC1 -- Given a local host with a tenant holding one `host.table.*` table
//! of 3 rows and one doc, When `host.export` completes and the archive is
//! listed, Then no entry path starts with `tables/` or `docs/` on the
//! pre-change code (version-1, array manifest) and the entry list is
//! printed. Once the archive is version 2 (AC2), the same test asserts the
//! falsifier stays refuted: the baseline was real and has since moved.

use crate::common;
use crate::timp;

use common::TestServer;
use serde_json::Value;

#[tokio::test]
async fn pre_change_archive_has_no_tables_or_docs_entries() {
    let server = TestServer::start().await;
    let t = timp::new_tenant(&server, "Timp AC1 Tenant").await;
    t.seed_table(3).await;
    t.seed_doc("a.md", "# a doc").await;

    let (_run_id, entries) = t.export(&server).await;
    let listing: Vec<&String> = entries.keys().collect();
    println!("archive entries: {listing:#?}");

    let manifest: Value = serde_json::from_slice(&entries["manifest.json"]).expect("manifest.json is JSON");
    let tabled = listing.iter().any(|p| p.starts_with("tables/"));
    let docd = listing.iter().any(|p| p.starts_with("docs/"));
    if manifest.is_array() {
        // Pre-change layout: the falsifier ("the rows are already exported")
        // is refuted only if neither prefix is present.
        assert!(!tabled, "falsifier confirmed: table rows already exported: {listing:?}");
        assert!(!docd, "falsifier confirmed: docs already exported: {listing:?}");
    } else {
        assert_eq!(manifest["version"], serde_json::json!(2), "manifest: {manifest}");
        assert!(tabled && docd, "version-2 archive must carry tables/ and docs/: {listing:?}");
    }
}
