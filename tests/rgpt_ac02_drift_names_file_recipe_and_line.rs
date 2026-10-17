//! PRD-mcphost-recipe-golden-path-tests
//! AC2 -- Given a hand edit of one line inside a generated llms.txt block,
//! When `cargo test rgpt_drift`, Then it fails naming `www/llms.txt`, the
//! recipe and the line; an edit outside the markers passes.
//!
//! `rgpt_drift_tree_matches_generator` is the check itself, run over the
//! real tree on every PR. The other two prove it can fail: they run the same
//! check over a scratch copy of the generated tree with one line edited.

use crate::common;

use common::TempDataDir;
use mcphost::recipes;
use std::path::Path;

fn repo() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// A scratch root holding `recipes/` and every generated file, freshly
/// rendered (never copied from the tree, so a drifted tree cannot hide).
fn scratch_generated() -> TempDataDir {
    let scratch = TempDataDir::new();
    std::fs::create_dir_all(scratch.0.join("recipes")).expect("mkdir recipes");
    for r in recipes::load_all(repo()).expect("load recipes") {
        let rel = format!("recipes/{}.toml", r.name);
        std::fs::copy(repo().join(&rel), scratch.0.join(&rel)).expect("copy toml");
    }
    std::fs::create_dir_all(scratch.0.join("www")).expect("mkdir www");
    std::fs::copy(repo().join("www/llms.txt"), scratch.0.join("www/llms.txt")).expect("copy llms.txt");
    let files = recipes::plan(&scratch.0).expect("plan");
    recipes::write(&scratch.0, &files).expect("write");
    scratch
}

#[test]
fn rgpt_drift_tree_matches_generator() {
    let drifted = recipes::drift(repo()).expect("drift check runs");
    assert!(
        drifted.is_empty(),
        "hand-edited recipe artifacts (run `cargo xtask recipes`):\n{}",
        drifted.join("\n")
    );
}

#[test]
fn rgpt_drift_edit_inside_a_block_names_file_recipe_and_line() {
    let scratch = scratch_generated();
    assert!(recipes::drift(&scratch.0).expect("drift").is_empty(), "fresh scratch must not drift");

    let path = scratch.0.join("www/llms.txt");
    let text = std::fs::read_to_string(&path).expect("read");
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    let begin = lines
        .iter()
        .position(|l| *l == recipes::begin_marker("chart-in-a-minute"))
        .expect("chart-in-a-minute block");
    // First numbered call line inside the block.
    let at = (begin..lines.len()).find(|i| lines[*i].starts_with("1. `")).expect("a call line");
    lines[at].push_str(" (hand edit)");
    std::fs::write(&path, lines.join("\n")).expect("write");

    let drifted = recipes::drift(&scratch.0).expect("drift");
    assert_eq!(drifted.len(), 1, "{drifted:?}");
    let msg = &drifted[0];
    assert!(msg.contains("www/llms.txt"), "{msg}");
    assert!(msg.contains("chart-in-a-minute"), "{msg}");
    assert!(msg.contains(&format!("line {}", at + 1)), "want line {} in: {msg}", at + 1);
    assert!(msg.contains("(hand edit)"), "{msg}");
}

#[test]
fn rgpt_drift_edit_outside_the_markers_passes() {
    let scratch = scratch_generated();
    let path = scratch.0.join("www/llms.txt");
    let text = std::fs::read_to_string(&path).expect("read");
    let begin = text.find(&recipes::begin_marker("chart-in-a-minute")).expect("block");
    // Hand-written prose above the block, and a new line below the file's top.
    let edited = format!("{}\nA hand-written line outside every marker.\n{}", &text[..begin], &text[begin..]);
    std::fs::write(&path, edited).expect("write");
    let drifted = recipes::drift(&scratch.0).expect("drift");
    assert!(drifted.is_empty(), "an edit outside the markers must pass: {drifted:?}");
}
