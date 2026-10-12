//! PRD-mcphost-export-tables-and-import-restore
//! AC7 -- Given the `ArchiveEntry` enum, When the test renders the
//! exporter's paths, the importer's accepted paths, and `docs/tools.md`'s
//! layout table, Then the three sets are identical.

use crate::common;
use crate::timp;

use std::collections::BTreeSet;

use common::TestServer;
use mcphost::export::ArchiveEntry;

/// Backticked first cell of every row under `## Archive layout`.
fn doc_layout(tools_md: &str) -> BTreeSet<String> {
    let section = tools_md
        .split_once(mcphost::gendocs::ARCHIVE_LAYOUT_HEADING.trim_start())
        .expect("docs/tools.md has the archive-layout section")
        .1;
    section
        .lines()
        .filter(|l| l.starts_with("| `"))
        .map(|l| l.trim_start_matches("| `").split('`').next().expect("cell").to_string())
        .collect()
}

#[tokio::test]
async fn exporter_importer_and_docs_agree_on_the_entry_layout() {
    // Exporter: every path a real archive carries, as a layout pattern ...
    let server = TestServer::start().await;
    let t = timp::new_tenant(&server, "Timp AC7 Tenant").await;
    t.seed_full().await;
    let (_run, entries) = t.export(&server).await;
    let exported: BTreeSet<String> = entries
        .keys()
        .map(|p| ArchiveEntry::parse(p).unwrap_or_else(|| panic!("exporter wrote '{p}', which the importer rejects")).layout_pattern())
        .collect();

    // ... against the enum's own rendering of every kind (the importer's
    // accepted set: each rendered path must parse back to its own kind) ...
    let rendered: BTreeSet<String> = ArchiveEntry::layout().into_iter().map(|(p, _)| p).collect();
    for pattern in &rendered {
        let parsed = ArchiveEntry::parse(pattern).unwrap_or_else(|| panic!("importer rejects its own layout path {pattern}"));
        assert_eq!(&parsed.layout_pattern(), pattern);
        assert_eq!(&parsed.path(), pattern);
    }
    for foreign in ["tools/x/v2/config.json", "tables/t/other.json", "state/k", "models/", "cache/x", "docs/"] {
        assert!(ArchiveEntry::parse(foreign).is_none(), "importer must not accept '{foreign}'");
    }

    // ... against the table in the committed docs/tools.md.
    let tools_md = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/tools.md")).expect("docs/tools.md");
    let documented = doc_layout(&tools_md);

    assert_eq!(rendered, documented, "docs/tools.md layout table differs from the enum");
    // The tenant has no python tool, so no tools/<name>/v1/source in its archive.
    let mut expected_exported = rendered.clone();
    expected_exported.remove("tools/<name>/v1/source");
    assert_eq!(exported, expected_exported, "exporter paths differ from the enum's layout");
}
