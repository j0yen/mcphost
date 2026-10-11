//! PRD-mcphost-docs-examples-executed-nightly AC2 (P0) -- Given a page with
//! a `mcphost-tool kind=python name=t1` fence followed by a `mcphost-call
//! expect=contains:hello` fence, When `extract` runs, Then the corpus has
//! two entries in that order with ids `<path>#<heading>-1` and `-2`, and
//! `--check` exits 0; after editing the fence body, `--check` exits 1
//! naming the id.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn temp_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dxn-ac02-{tag}-{}-{}", std::process::id(), mcphost::state::now_unix_ms()));
    std::fs::create_dir_all(dir.join("docs")).unwrap();
    dir
}

fn docs_examples(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .arg("docs-examples")
        .args(args)
        .arg("--root")
        .arg(root)
        .output()
        .expect("run mcphost docs-examples")
}

const PAGE: &str = "# Greeter\n\nSome prose.\n\n## Say hello\n\n```python mcphost-tool kind=python name=t1\ndef main(args):\n    return {\"msg\": \"hello\"}\n```\n\nCall it:\n\n```json mcphost-call expect=contains:hello\n{\"name\": \"host.tool_test\", \"arguments\": {\"name\": \"t1\", \"args\": {}}}\n```\n\n```\nuntagged fences are ignored\n```\n";

#[test]
fn extract_orders_entries_and_check_names_the_drifted_id() {
    let root = temp_root("order");
    std::fs::write(root.join("docs/page.md"), PAGE).unwrap();

    let out = docs_examples(&root, &["extract"]);
    assert!(out.status.success(), "extract failed: {}", String::from_utf8_lossy(&out.stderr));

    let corpus: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("docs/examples.corpus.json")).unwrap()).unwrap();
    let entries = corpus.as_array().unwrap();
    assert_eq!(entries.len(), 2, "untagged fences are ignored: {corpus}");
    assert_eq!(entries[0]["id"], "docs/page.md#say-hello-1");
    assert_eq!(entries[0]["tag"], "mcphost-tool");
    assert_eq!(entries[0]["attrs"]["name"], "t1");
    assert_eq!(entries[1]["id"], "docs/page.md#say-hello-2");
    assert_eq!(entries[1]["tag"], "mcphost-call");
    assert_eq!(entries[1]["expect"], "contains:hello");

    let check = docs_examples(&root, &["extract", "--check"]);
    assert_eq!(check.status.code(), Some(0), "fresh corpus: {}", String::from_utf8_lossy(&check.stderr));

    let edited = PAGE.replace("\"msg\": \"hello\"", "\"msg\": \"goodbye\"");
    std::fs::write(root.join("docs/page.md"), edited).unwrap();
    let check = docs_examples(&root, &["extract", "--check"]);
    assert_eq!(check.status.code(), Some(1), "edited body must fail --check");
    let stderr = String::from_utf8_lossy(&check.stderr);
    assert!(stderr.contains("docs/page.md#say-hello-1"), "--check must name the drifted id: {stderr}");
    assert!(!stderr.contains("docs/page.md#say-hello-2"), "unchanged entry must not be named: {stderr}");
    std::fs::remove_dir_all(&root).ok();
}
