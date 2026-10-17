//! PRD-mcphost-delegation-recipes-and-quickstart
//! AC3 (P0) — Given `mcphost llms-txt`, When its output is diffed against
//! `www/llms.txt`, Then the only generated sections are Tools, Scopes and
//! "Give a sub-agent one key, not yours", and the latter precedes
//! "Share a tool, not a key".

use mcphost::llms_txt::{RECIPE_END, RECIPE_START, SCOPES_END, SCOPES_START, TOOLS_SECTION_END, TOOLS_SECTION_START};
use std::path::Path;
use std::process::Command;

const LLMS_TXT: &str = include_str!("../www/llms.txt");
const ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// Replace each block's body (markers kept) with `STALE`.
fn staled(content: &str, blocks: &[(&str, &str)]) -> String {
    let mut out = content.to_string();
    for (start, end) in blocks {
        let s = out.find(start).expect("start marker") + start.len();
        let e = out.find(end).expect("end marker");
        out = format!("{}\nSTALE\n{}", &out[..s], &out[e..]);
    }
    out
}

/// Run `mcphost llms-txt` over copies of the repo files with `llms` as the
/// llms.txt; return the regenerated llms.txt.
fn regenerate(dir: &Path, llms: &str) -> String {
    let root = Path::new(ROOT);
    std::fs::write(dir.join("llms.txt"), llms).unwrap();
    std::fs::copy(root.join("README.md"), dir.join("README.md")).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .current_dir(root)
        .args(["llms-txt", "--path"])
        .arg(dir.join("llms.txt"))
        .arg("--readme")
        .arg(dir.join("README.md"))
        .output()
        .expect("run mcphost llms-txt");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    std::fs::read_to_string(dir.join("llms.txt")).unwrap()
}

#[test]
fn dlrec_ac03_only_tools_scopes_and_recipe_are_generated() {
    let dir = std::env::temp_dir().join(format!("dlrec_ac03_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let blocks = [
        (TOOLS_SECTION_START, TOOLS_SECTION_END),
        (SCOPES_START, SCOPES_END),
        (RECIPE_START, RECIPE_END),
    ];

    // Committed file is its own regeneration.
    assert_eq!(regenerate(&dir, LLMS_TXT), LLMS_TXT, "run `mcphost llms-txt`");

    // Staling exactly the three blocks and regenerating restores the file
    // byte-for-byte: those blocks are the generated sections the diff touches.
    let stale = staled(LLMS_TXT, &blocks);
    assert_ne!(stale, LLMS_TXT);
    assert_eq!(regenerate(&dir, &stale), LLMS_TXT);

    // Staling the hand-written text instead is never repaired.
    let hand = LLMS_TXT.replacen("## Give your agents one memory", "## Give your agents one memory (edited)", 1);
    assert_eq!(regenerate(&dir, &hand), hand);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dlrec_ac03_recipe_precedes_share_a_tool() {
    let recipe = LLMS_TXT.find("## Give a sub-agent one key, not yours").expect("recipe heading");
    let share = LLMS_TXT.find("## Share a tool, not a key").expect("share heading");
    assert!(recipe < share);
    assert!(LLMS_TXT.find(SCOPES_START).unwrap() < recipe, "Scopes table precedes the recipe");
}
