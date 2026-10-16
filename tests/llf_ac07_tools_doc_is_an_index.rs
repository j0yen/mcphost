//! PRD-mcphost-llms-full-generated-from-schemas AC7: the regenerated
//! `docs/tools.md` carries no `input_schema` text and every tool row links
//! to `www/llms-full.txt#<slug>`.

use mcphost::kinds::KindRegistry;

#[test]
fn llf_ac07() {
    let committed = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/tools.md")).expect("docs/tools.md");
    assert_eq!(
        committed,
        mcphost::gendocs::render_tools_markdown(&KindRegistry::with_builtin()),
        "docs/tools.md is stale: run `mcphost gen-docs`"
    );
    assert!(!committed.contains("input_schema"), "docs/tools.md must not copy schemas");
    let table = committed.split("\n## Archive layout").next().unwrap();
    let rows: Vec<&str> = table.lines().filter(|l| l.starts_with("| `")).collect();
    assert!(!rows.is_empty());
    for row in rows {
        // Descriptions carry escaped `\|`, so read the first and last cells by position.
        let name = row.trim_start_matches("| ").split('`').nth(1).unwrap();
        let last = row.trim_end().trim_end_matches('|').rsplit(" | ").next().unwrap();
        let want = format!("(www/llms-full.txt#{})", mcphost::llms_txt::anchor_slug(name));
        assert!(last.contains(&want), "{name}: row does not link {want}: {last}");
    }
}
