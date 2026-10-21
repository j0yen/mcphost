//! contract: reads docs/tools.md, www/llms.txt
//! PRD-mcphost-sync-source-registry-and-create AC2 -- Given `SourceKind::ALL`,
//! When a test renders the tool schema, `docs/tools.md`, `www/llms.txt` and
//! reads the puller dispatch arms, Then every kind appears in each exactly
//! once and `gen-docs --check` exits 1 naming the file after any one copy is
//! hand-edited.
//!
//! The `--check` half runs the real binary against a scratch copy of `docs/`,
//! `www/` and the root `*.md` files, never the working tree.

use mcphost::kinds::KindRegistry;
use mcphost::sync::{SourceKind, kinds_list};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

/// Occurrences of `needle` as a whole word (`_` counts as a word character).
fn words(hay: &str, needle: &str) -> usize {
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    hay.match_indices(needle)
        .filter(|(i, _)| {
            let before = hay[..*i].chars().next_back();
            let after = hay[*i + needle.len()..].chars().next();
            !before.is_some_and(is_word) && !after.is_some_and(is_word)
        })
        .count()
}

fn assert_once_each(label: &str, hay: &str) {
    for k in SourceKind::ALL {
        assert_eq!(words(hay, k.as_str()), 1, "{label}: kind `{}` must appear exactly once", k.as_str());
    }
}

#[test]
fn syreg_ac02_every_kind_appears_exactly_once_in_every_copy() {
    assert_eq!(kinds_list(), SourceKind::names().join(", "));

    // The tool schema's `source.kind` enum.
    let kinds = KindRegistry::with_builtin();
    let create = mcphost::handler::host_tool_descriptors(&kinds)
        .into_iter()
        .find(|t| t.name == "host.sync.create")
        .expect("host.sync.create is advertised");
    let schema = serde_json::to_string(&*create.input_schema).expect("schema json");
    assert_once_each("host.sync.create input schema", &schema);

    // The two rendered documents.
    assert_once_each("docs/tools.md", &read("docs/tools.md"));
    assert_once_each("www/llms.txt", &read("www/llms.txt"));
    let section = mcphost::sync::render_llms_section();
    assert_once_each("rendered `## Data sync` section", &section);
    assert!(read("www/llms.txt").contains(&section), "www/llms.txt carries the rendered section verbatim");

    // The puller dispatch: the arms of `puller_for`, one per kind.
    let src = read("src/sync/mod.rs");
    let start = src.find("pub const fn puller_for(").expect("puller_for exists");
    let body = &src[start..];
    let arms = &body[..body.find("\n}\n").expect("puller_for ends")];
    for k in SourceKind::ALL {
        let variant = format!("SourceKind::{} =>", variant_name(*k));
        assert_eq!(arms.matches(&variant).count(), 1, "puller_for: exactly one arm for {}", k.as_str());
        assert!(!mcphost::sync::puller_for(*k).is_empty());
    }
    assert_eq!(arms.matches("SourceKind::").count(), SourceKind::ALL.len(), "no arm for a kind outside ALL");
}

/// The enum variant behind each kind, spelled by an exhaustive match so a new
/// kind fails to compile here until this test is taught its arm.
fn variant_name(k: SourceKind) -> &'static str {
    match k {
        SourceKind::CsvUrl => "CsvUrl",
        SourceKind::GoogleSheet => "GoogleSheet",
        SourceKind::Airtable => "Airtable",
        SourceKind::NotionDb => "NotionDb",
        SourceKind::HttpJson => "HttpJson",
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create scratch dir");
    for entry in std::fs::read_dir(from).expect("read dir") {
        let entry = entry.expect("dir entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy file");
        }
    }
}

fn scratch(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("syreg_ac02_{label}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(&root().join("docs"), &dir.join("docs"));
    copy_dir(&root().join("www"), &dir.join("www"));
    // Root-level files too (README.md, llms-install.md): `gen-docs` renders both.
    for entry in std::fs::read_dir(root()).expect("read repo root") {
        let entry = entry.expect("root entry");
        if entry.file_type().expect("file type").is_file() && entry.file_name().to_string_lossy().ends_with(".md") {
            std::fs::copy(entry.path(), dir.join(entry.file_name())).expect("copy root doc");
        }
    }
    dir
}

fn gen_docs_check(dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["gen-docs", "--check"])
        .current_dir(dir)
        .output()
        .expect("run gen-docs --check")
}

#[test]
fn syreg_ac02_gen_docs_check_names_a_hand_edited_copy() {
    let dir = scratch("check");
    let clean = gen_docs_check(&dir);
    assert!(clean.status.success(), "fresh copy passes: {}", String::from_utf8_lossy(&clean.stderr));

    for (rel, from, to) in [
        ("www/llms.txt", "- `airtable` --", "- `airtable_hand_edit` --"),
        ("docs/tools.md", "`airtable`,", "`airtable_hand_edit`,"),
    ] {
        let path = dir.join(rel);
        let text = std::fs::read_to_string(&path).expect("read copy");
        assert!(text.contains(from), "{rel}: the copy carries `{from}`");
        let edited = text.replacen(from, to, 1);
        assert_ne!(text, edited, "{rel}: the edit must change the copy");
        std::fs::write(&path, &edited).expect("write edit");
        let out = gen_docs_check(&dir);
        assert_eq!(out.status.code(), Some(1), "{rel}: --check exits 1 after a hand edit");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(rel), "{rel}: --check names the file: {stderr}");
        std::fs::write(&path, &text).expect("restore");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
