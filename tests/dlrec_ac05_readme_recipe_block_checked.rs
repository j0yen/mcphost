//! PRD-mcphost-delegation-recipes-and-quickstart
//! AC5 (P1) — Given `README.md`, When `scripts/sandbox-api-doc-check.sh` runs
//! without `--write`, Then it exits non-zero iff the block between
//! `<!-- recipe:sub_agent_key -->` markers differs from the rendered recipe.

use mcphost::llms_txt::{RECIPE_END, RECIPE_START, render_recipe_block, splice_readme_recipe};
use mcphost::recipes::sub_agent_key;
use std::path::Path;
use std::process::{Command, Output};

const README: &str = include_str!("../README.md");
const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn check(readme: &Path) -> Output {
    Command::new("bash")
        .arg(Path::new(ROOT).join("scripts/sandbox-api-doc-check.sh"))
        .env("MCPHOST_BIN", env!("CARGO_BIN_EXE_mcphost"))
        .env("MCPHOST_README", readme)
        .output()
        .expect("run sandbox-api-doc-check.sh")
}

fn scratch(name: &str, content: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("dlrec_ac05_{}_{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("README.md");
    std::fs::write(&p, content).unwrap();
    p
}

#[test]
fn dlrec_ac05_committed_block_equals_the_render_and_passes() {
    let block = render_recipe_block(&sub_agent_key());
    assert!(README.contains(&block), "README.md must carry the rendered block; run `mcphost llms-txt`");
    assert!(README.contains("### Sub-agents"));
    let out = check(&Path::new(ROOT).join("README.md"));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn dlrec_ac05_a_drifted_block_exits_non_zero_naming_the_recipe() {
    let start = README.find(RECIPE_START).unwrap();
    let end = README.find(RECIPE_END).unwrap();
    assert!(README[start..end].contains("revoked"));
    let drifted = format!("{}{}{}", &README[..start], README[start..end].replacen("revoked", "revoked-edited", 1), &README[end..]);
    assert_ne!(drifted, README);
    let out = check(&scratch("drift", &drifted));
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("recipe:sub_agent_key"));
}

#[test]
fn dlrec_ac05_a_missing_block_exits_non_zero() {
    let start = README.find(RECIPE_START).unwrap();
    let end = README.find(RECIPE_END).unwrap() + RECIPE_END.len();
    let missing = format!("{}{}", &README[..start], &README[end..]);
    assert!(!check(&scratch("missing", &missing)).status.success());
}

#[test]
fn dlrec_ac05_an_edit_outside_the_block_still_passes() {
    let edited = README.replacen("# mcphost", "# mcphost (edited)", 1);
    assert_ne!(edited, README);
    let out = check(&scratch("outside", &edited));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn dlrec_ac05_splice_is_idempotent_and_inserts_under_a_heading() {
    let block = render_recipe_block(&sub_agent_key());
    assert_eq!(splice_readme_recipe(README, &block), README);
    let start = README.find("### Sub-agents").unwrap();
    let end = README.find(RECIPE_END).unwrap() + RECIPE_END.len();
    let without = format!("{}{}", &README[..start], README[end..].trim_start_matches('\n'));
    assert_eq!(splice_readme_recipe(&without, &block), README);
}
