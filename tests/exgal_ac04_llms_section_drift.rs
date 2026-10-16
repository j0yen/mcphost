//! AC4: the examples section of `www/llms.txt` (and the sitemap block) is a
//! pure render of the manifests. A hand edit of one line inside the markers
//! fails naming the line; an edit outside the markers passes; `mcphost
//! examples index` run twice changes zero bytes.

use std::path::{Path, PathBuf};
use std::process::Command;

use mcphost::examples::{collect, registered_kind_names};
use mcphost::examples_page::{SectionRow, render_llms_section, render_sitemap_section, section_drift};

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn expected_section() -> String {
    let entries = collect(&root().join("examples"), &registered_kind_names()).expect("manifests are valid");
    let rows: Vec<SectionRow> = entries.iter().map(SectionRow::from).collect();
    render_llms_section(&rows)
}

#[test]
fn exgal_ac04_committed_section_matches_the_render() {
    let committed = std::fs::read_to_string(root().join("www/llms.txt")).unwrap();
    section_drift(&committed, &expected_section()).unwrap_or_else(|e| panic!("{e}"));

    let entries = collect(&root().join("examples"), &registered_kind_names()).unwrap();
    let names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
    let sitemap = std::fs::read_to_string(root().join("www/sitemap.xml")).unwrap();
    section_drift(&sitemap, render_sitemap_section(&names).trim()).unwrap_or_else(|e| panic!("sitemap: {e}"));
}

#[test]
fn exgal_ac04_edit_inside_markers_fails_naming_the_line() {
    let committed = std::fs::read_to_string(root().join("www/llms.txt")).unwrap();
    let victim = "- uptime-probes — Uptime probes with no server";
    assert!(committed.contains(victim));
    let edited = committed.replace(victim, "- uptime-probes — Uptime probes with a SERVER");
    let err = section_drift(&edited, &expected_section()).expect_err("a hand edit inside the markers must fail");
    assert!(err.contains("Uptime probes with a SERVER"), "names the edited line: {err}");
    assert!(err.contains("line "), "names the line number: {err}");
}

#[test]
fn exgal_ac04_edit_outside_markers_passes() {
    let committed = std::fs::read_to_string(root().join("www/llms.txt")).unwrap();
    let edited = committed.replacen("## Full documentation", "## Full documentation (edited by hand)", 1);
    assert_ne!(edited, committed);
    section_drift(&edited, &expected_section()).expect("an edit outside the markers is not drift");
}

fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("exgal-ac04-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("www")).unwrap();
    let st = Command::new("cp").arg("-r").arg(root().join("examples")).arg(dir.join("examples")).status().unwrap();
    assert!(st.success());
    for f in ["llms.txt", "sitemap.xml"] {
        std::fs::copy(root().join("www").join(f), dir.join("www").join(f)).unwrap();
    }
    dir
}

fn run_index(dir: &Path) {
    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["examples", "index", "--dir"])
        .arg(dir.join("examples"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn exgal_ac04_examples_index_twice_changes_zero_bytes() {
    let dir = scratch();
    let files = ["examples/index.json", "www/llms.txt", "www/sitemap.xml"];
    run_index(&dir);
    let first: Vec<Vec<u8>> = files.iter().map(|f| std::fs::read(dir.join(f)).unwrap()).collect();
    run_index(&dir);
    for (f, before) in files.iter().zip(&first) {
        assert_eq!(&std::fs::read(dir.join(f)).unwrap(), before, "{f} changed on the second run");
        assert_eq!(&std::fs::read(dir.join(f)).unwrap(), &std::fs::read(root().join(f)).unwrap(), "{f} differs from the committed file");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
