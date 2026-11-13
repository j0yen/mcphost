//! PRD-mcphost-export-data-class-completeness-v2
//! AC7 -- Given `docs/tools.md`, When the layout render test runs, Then the
//! committed `## Archive layout` table and excluded-classes table equal the
//! render of `ArchiveEntry::layout()` and `EXPORT_EXCLUSIONS`.

use mcphost::export::{ArchiveEntry, EXCLUDED_CLASSES_HEADING, EXPORT_EXCLUSIONS};

/// The table lines (`| ... |`) between `start` and the next heading.
fn table_after(doc: &str, start: &str) -> String {
    let after = doc.split_once(start).unwrap_or_else(|| panic!("docs/tools.md lacks {start:?}")).1;
    let section = after.split("\n#").next().expect("section");
    let mut out: String = section.lines().filter(|l| l.starts_with('|')).collect::<Vec<_>>().join("\n");
    out.push('\n');
    out
}

#[test]
fn expc_ac07() {
    let doc = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/tools.md")).expect("docs/tools.md");

    // The layout table, rendered here from the enum.
    let mut layout = String::from("| path | host.import |\n|---|---|\n");
    for (pattern, note) in ArchiveEntry::layout() {
        layout.push_str(&format!("| `{pattern}` | {note} |\n"));
    }
    assert_eq!(
        table_after(&doc, mcphost::gendocs::ARCHIVE_LAYOUT_HEADING.trim_start()),
        layout,
        "the committed `## Archive layout` table differs from ArchiveEntry::layout() (run `mcphost gen-docs`)"
    );

    // The excluded-classes table, rendered here from EXPORT_EXCLUSIONS.
    let mut excluded = String::from("| table | reason |\n|---|---|\n");
    for (table, why) in EXPORT_EXCLUSIONS {
        excluded.push_str(&format!("| `{table}` | {} |\n", why.reason()));
    }
    assert_eq!(
        table_after(&doc, EXCLUDED_CLASSES_HEADING.trim_start()),
        excluded,
        "the committed excluded-classes table differs from EXPORT_EXCLUSIONS (run `mcphost gen-docs`)"
    );
}
