//! PRD-mcphost-csv-to-table-one-call
//! AC7 -- Given `gendocs run --check` (`mcphost gen-docs --check`), When the
//! llms.txt recipe section or the verb's schema is edited by hand, Then the
//! check fails naming the drifted region.
//!
//! The real binary runs `gen-docs --check` in a scratch copy of the files it
//! reads: a pristine copy passes, a hand-edited llms.txt recipe region and a
//! hand-edited `schemas/table-from-blob.args.json` each fail with a line
//! naming that region. The source of both regions is `csvtab::ARG_SPECS`
//! (one `TableFromBlobArgs` descriptor), pinned by the unit test
//! `csvtab::tests::arg_specs_match_the_struct_fields`.

use mcphost::csvtab;
use std::path::{Path, PathBuf};
use std::process::Command;

const FILES: &[&str] = &[
    "README.md",
    "docs/agent-quickstart.md",
    "docs/plans.md",
    "docs/tools.md",
    "www/llms.txt",
    "www/plans.html",
    "www/llms-install.md",
    "llms-install.md",
    csvtab::SCHEMA_FILE,
];

fn scratch_copy(label: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = std::env::temp_dir().join(format!("mcphost-csvtab-{label}-{}-{}", std::process::id(), mcphost::state::now_unix_ms()));
    for f in FILES {
        let to = dir.join(f);
        std::fs::create_dir_all(to.parent().expect("parent")).expect("mkdir");
        std::fs::copy(root.join(f), &to).unwrap_or_else(|e| panic!("copy {f}: {e}"));
    }
    dir
}

fn gen_docs_check(dir: &Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["gen-docs", "--check"])
        .current_dir(dir)
        .env_remove("MCPHOST_SUPPORT_URL")
        .output()
        .expect("run mcphost gen-docs --check");
    (out.status.success(), String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
fn pristine_files_pass_the_check() {
    let dir = scratch_copy("ac07-ok");
    let (ok, stderr) = gen_docs_check(&dir);
    assert!(ok, "pristine copy must pass: {stderr}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn hand_edited_llms_txt_recipe_region_fails_naming_it() {
    let dir = scratch_copy("ac07-llms");
    let p = dir.join("www/llms.txt");
    let text = std::fs::read_to_string(&p).expect("read");
    assert!(text.contains(csvtab::RECIPE_START) && text.contains(csvtab::RECIPE_END), "llms.txt carries the csvtab markers");
    // A hand edit inside the region: invent an argument the verb does not take.
    let edited = text.replacen("`dry_run` (boolean, default false)", "`dry_run` (boolean, default false), `nonexistent_arg` (string)", 1);
    assert_ne!(edited, text, "the edit must change the file");
    std::fs::write(&p, edited).expect("write");
    let (ok, stderr) = gen_docs_check(&dir);
    assert!(!ok, "a hand-edited recipe region must fail the check");
    assert!(stderr.contains("www/llms.txt region csvtab"), "must name the drifted region: {stderr}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn hand_edited_verb_schema_fails_naming_it() {
    let dir = scratch_copy("ac07-schema");
    let p = dir.join(csvtab::SCHEMA_FILE);
    let text = std::fs::read_to_string(&p).expect("read");
    let edited = text.replacen("\"additionalProperties\": false", "\"additionalProperties\": true", 1);
    assert_ne!(edited, text, "the edit must change the file");
    std::fs::write(&p, edited).expect("write");
    let (ok, stderr) = gen_docs_check(&dir);
    assert!(!ok, "a hand-edited schema must fail the check");
    assert!(stderr.contains(csvtab::SCHEMA_FILE), "must name the drifted schema file: {stderr}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn drift_is_computed_from_the_one_descriptor() {
    let llms = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("www/llms.txt")).expect("llms.txt");
    let schema = csvtab::render_schema_file();
    assert!(csvtab::drifted_regions(&llms, &schema).is_empty(), "committed files match a fresh render");
    // Every argument the verb takes appears in the recipe, and the schema names exactly them.
    for a in csvtab::ARG_SPECS {
        assert!(llms.contains(&format!("`{}`", a.name)), "llms.txt recipe must name argument {}", a.name);
    }
    let v: serde_json::Value = serde_json::from_str(&schema).expect("schema json");
    let props = v["properties"].as_object().expect("properties");
    assert_eq!(props.len(), csvtab::ARG_SPECS.len());
    assert!(!csvtab::drifted_regions(&llms, "{}\n").is_empty());
}
