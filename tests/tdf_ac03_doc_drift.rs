//! PRD-mcphost-tool-directory-format
//! AC3 (P0) — Given `docs/tool-directory.md` edited by hand or a spec field
//! added to any kind, When `cargo test` runs, Then the drift test fails
//! naming the differing line.

use mcphost::kinds::production_registry;
use mcphost::tooldir::{DOC_PATH, check_doc, render_docs};

fn committed() -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(DOC_PATH)).expect("docs/tool-directory.md")
}

#[test]
fn tdf_ac03_committed_doc_matches_the_registry_render() {
    if let Err(why) = check_doc(&production_registry(), &committed()) {
        panic!("{why}");
    }
}

#[test]
fn tdf_ac03_hand_edit_is_caught_naming_the_line() {
    let registry = production_registry();
    let mut lines: Vec<String> = committed().lines().map(str::to_string).collect();
    let idx = lines.iter().position(|l| l.starts_with("| echo ")).expect("an echo row");
    lines[idx] = format!("{} `hand_edited`", lines[idx]);
    let edited = lines.join("\n") + "\n";
    let err = check_doc(&registry, &edited).expect_err("a hand edit must fail the drift check");
    assert!(err.contains(&format!("line {}", idx + 1)), "must name line {}: {err}", idx + 1);
    assert!(err.contains("hand_edited"), "must show the differing line: {err}");
}

#[test]
fn tdf_ac03_doc_lists_every_known_spec_field() {
    let doc = render_docs(&production_registry());
    let registry = production_registry();
    for name in registry.names() {
        for field in registry.get(name).unwrap().known_spec_fields() {
            assert!(doc.contains(&format!("`{field}`")), "{name}.{field} missing from the generated doc");
        }
    }
}
