//! PRD-mcphost-contract-output-schemas
//! AC7 (P1) -- Given `docs/tools.md` regenerated, When
//! `host.tool.publish`'s section is read, Then it has a `Returns` block
//! listing `version` and `call_line` with their types.

use mcphost::kinds::KindRegistry;

fn section<'a>(doc: &'a str, tool: &str) -> &'a str {
    let start = doc
        .find(&format!("### `{tool}`\n"))
        .unwrap_or_else(|| panic!("no section for {tool}"));
    let rest = &doc[start..];
    let end = rest[4..].find("\n### ").map_or(rest.len(), |i| i + 4);
    &rest[..end]
}

#[test]
fn host_tool_publish_section_lists_version_and_call_line_with_types() {
    let generated = mcphost::gendocs::render_tools_markdown(&KindRegistry::with_builtin());
    let committed = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/tools.md"))
        .expect("docs/tools.md");
    assert_eq!(generated, committed, "docs/tools.md is stale: run `mcphost gen-docs`");

    let publish = section(&committed, "host.tool.publish");
    assert!(publish.contains("**Returns**"), "{publish}");
    assert!(publish.contains("- `version`: integer"), "{publish}");
    assert!(publish.contains("- `call_line`: string"), "{publish}");
}

#[test]
fn every_canonical_tool_has_a_returns_block() {
    let doc = mcphost::gendocs::render_tools_markdown(&KindRegistry::with_builtin());
    for name in mcphost::handler::canonical_control_plane_names(&KindRegistry::with_builtin()) {
        assert!(section(&doc, &name).contains("**Returns**"), "{name} has no Returns block");
    }
}
