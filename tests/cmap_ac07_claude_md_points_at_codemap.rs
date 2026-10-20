//! PRD-mcphost-generated-code-map
//! AC7 — Given the repo root, When `CLAUDE.md` is read, Then it is under 30 lines
//! and contains the strings `CODEMAP.md` and `scripts/gen-codemap.sh`, and that
//! script path exists and is executable.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

#[test]
fn cmap_ac07_claude_md_short_and_names_map_and_script() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(root.join("CLAUDE.md")).expect("CLAUDE.md");
    assert!(text.lines().count() < 30, "CLAUDE.md has {} lines", text.lines().count());
    assert!(text.contains("CODEMAP.md"));
    assert!(text.contains("scripts/gen-codemap.sh"));
    let mode = std::fs::metadata(root.join("scripts/gen-codemap.sh")).expect("script").permissions().mode();
    assert!(mode & 0o111 != 0, "scripts/gen-codemap.sh must be executable (mode {mode:o})");
}
