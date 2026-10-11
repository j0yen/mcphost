//! PRD-mcphost-blob-put-get-signed-url
//! AC1 — Given the synthorg corpus task block, When a test greps for a task
//! whose fixture is a binary file (`.pdf`, `.png`, `.xlsx`, `.zip`), Then the
//! count is printed; the builder added `recipe_file_share` (R7) so it is 1.
//!
//! The synthorg repo is outside this worktree, so the block ships as
//! `examples/file-share/synthorg-task.yaml` (same shape as the sibling
//! `examples/*/synthorg-task.yaml` tasks) and this test greps that file.

const BINARY_EXTS: [&str; 4] = [".pdf", ".png", ".xlsx", ".zip"];

#[test]
fn corpus_has_exactly_one_binary_fixture_task_recipe_file_share() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(root.join("examples/file-share/synthorg-task.yaml"))
        .expect("read examples/file-share/synthorg-task.yaml");
    let fixtures: Vec<&str> = text
        .lines()
        .filter_map(|l| l.trim().strip_prefix("fixture:"))
        .map(|v| v.trim().trim_matches('"'))
        .filter(|v| BINARY_EXTS.iter().any(|e| v.ends_with(e)))
        .collect();
    println!("binary-fixture task count: {}", fixtures.len());
    assert_eq!(fixtures.len(), 1, "expected recipe_file_share only");
    assert!(text.contains("id: recipe_file_share"));
    assert!(text.contains("recipe: file-share"));
    assert!(text.contains("spreadsheet_owner"));
    let fx = root.join(fixtures[0]);
    let len = std::fs::metadata(&fx).expect("fixture exists").len();
    assert_eq!(len, 300 * 1024, "fixture is 300 KB");
}
