//! PRD-mcphost-docs-examples-executed-nightly AC5 (P0) -- Given a
//! `mcphost-call` whose tool was published by a fence on a different page,
//! When `extract` runs, Then it exits 1 with `cross-page dependency
//! <id> -> <id>`.

use std::path::Path;
use std::process::{Command, Output};

fn extract(root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["docs-examples", "extract", "--root"])
        .arg(root)
        .output()
        .expect("run mcphost docs-examples extract")
}

#[test]
fn a_call_to_a_tool_published_on_another_page_fails_extraction() {
    let root = std::env::temp_dir().join(format!("dxn-ac05-{}-{}", std::process::id(), mcphost::state::now_unix_ms()));
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::write(
        root.join("docs/a.md"),
        "# A\n\n## Publish\n\n```json mcphost-tool kind=echo name=shared_tool\n{\"schema\": {\"type\": \"object\"}}\n```\n",
    )
    .unwrap();
    std::fs::write(
        root.join("docs/b.md"),
        "# B\n\n## Use\n\n```json mcphost-call\n{\"name\": \"host.tool_test\", \"arguments\": {\"name\": \"shared_tool\", \"args\": {}}}\n```\n",
    )
    .unwrap();

    let out = extract(&root);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "must fail extraction: {stderr}");
    assert!(
        stderr.contains("cross-page dependency docs/b.md#use-1 -> docs/a.md#publish-1"),
        "stderr must name both ids: {stderr}"
    );
    assert!(!root.join("docs/examples.corpus.json").exists(), "a failed extraction must not write a corpus");

    // The same pair on ONE page is fine.
    std::fs::remove_file(root.join("docs/a.md")).unwrap();
    std::fs::write(
        root.join("docs/b.md"),
        "# B\n\n## Use\n\n```json mcphost-tool kind=echo name=shared_tool\n{\"schema\": {\"type\": \"object\"}}\n```\n\n```json mcphost-call\n{\"name\": \"host.tool_test\", \"arguments\": {\"name\": \"shared_tool\", \"args\": {}}}\n```\n",
    )
    .unwrap();
    let out = extract(&root);
    assert_eq!(out.status.code(), Some(0), "same-page dependency: {}", String::from_utf8_lossy(&out.stderr));
    std::fs::remove_dir_all(&root).ok();
}
