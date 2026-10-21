//! PRD-mcphost-sync-source-registry-and-create AC2 (P0) -- Given
//! `SourceKind::ALL`, When a test renders the tool schema, `docs/tools.md`,
//! `www/llms.txt` and reads the puller dispatch arms, Then every kind appears
//! in each exactly once and `gen-docs --check` exits 1 naming the file after
//! any one copy is hand-edited.
//! contract: reads registry handler::host_tool_descriptors

use std::path::Path;
use std::process::{Command, Output};

use mcphost::kinds::KindRegistry;
use mcphost::sync::SourceKind;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn occurrences(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

/// `csv_url` -> `CsvUrl`: the variant ident a dispatch arm spells.
fn variant_ident(kind: &str) -> String {
    kind.split('_')
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect()
}

fn gen_docs_check(root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["gen-docs", "--check"])
        .current_dir(root)
        .output()
        .unwrap()
}

/// A scratch root: `docs/`, `www/` and the README copied (so a test can edit
/// them), everything else linked to the real tree.
fn scratch_root(tag: &str) -> std::path::PathBuf {
    let tmp = std::env::temp_dir().join(format!("syreg_ac02_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    for entry in std::fs::read_dir(ROOT).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if matches!(name.to_str(), Some("target" | ".git")) {
            continue;
        }
        let dest = tmp.join(&name);
        if matches!(name.to_str(), Some("docs" | "www")) {
            copy_dir(&entry.path(), &dest);
        } else if name.to_str() == Some("README.md") {
            std::fs::copy(entry.path(), &dest).unwrap();
        } else {
            std::os::unix::fs::symlink(entry.path(), &dest).unwrap();
        }
    }
    tmp
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &to.join(entry.file_name()));
        } else {
            std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
        }
    }
}

#[test]
fn syreg_ac02() {
    let names: Vec<&str> = SourceKind::ALL.iter().map(|k| k.as_str()).collect();
    assert_eq!(names.len(), 5, "{names:?}");

    // The tool schema: the `source.kind` enum is ALL, and the create
    // descriptor names each kind exactly once (the enum entry).
    let tools = mcphost::handler::host_tool_descriptors(&KindRegistry::with_builtin());
    let create = tools.iter().find(|t| t.name == "host.sync.create").expect("host.sync.create is advertised");
    let schema = serde_json::to_string(&*create.input_schema).unwrap();
    let in_enum: Vec<&str> = create.input_schema["properties"]["source"]["properties"]["kind"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(in_enum, names);
    let all_sync: String = tools
        .iter()
        .filter(|t| t.name.starts_with("host.sync."))
        .map(|t| format!("{}{}", t.description.clone().unwrap_or_default(), serde_json::to_string(&*t.input_schema).unwrap()))
        .collect();
    for kind in &names {
        assert_eq!(occurrences(&schema, &format!("\"{kind}\"")), 1, "schema: {kind}");
        assert_eq!(occurrences(&all_sync, kind), 1, "sync tool descriptors: {kind}");
    }

    // docs/tools.md and www/llms.txt.
    let tools_md = std::fs::read_to_string(format!("{ROOT}/docs/tools.md")).unwrap();
    // The generated per-verb argument list also renders the schema's
    // `source.kind` enum (a derived copy of the same registry, checked above);
    // the kinds table is the one hand-visible copy counted here.
    let tools_md: String = tools_md
        .lines()
        .filter(|l| !l.trim_start().starts_with("- `source.kind` enum:"))
        .collect::<Vec<_>>()
        .join("\n");
    let llms = std::fs::read_to_string(format!("{ROOT}/www/llms.txt")).unwrap();
    for kind in &names {
        // The connection column names a provider, which can share a kind's name,
        // so a kind is counted in the backticked form the tables render it in.
        assert_eq!(occurrences(&tools_md, &format!("`{kind}`")), 1, "docs/tools.md: {kind}");
        assert_eq!(occurrences(&llms, &format!("`{kind}`")), 1, "www/llms.txt: {kind}");
    }

    // The puller dispatch arms in src/sync/mod.rs.
    let source = std::fs::read_to_string(format!("{ROOT}/src/sync/mod.rs")).unwrap();
    let arms = source
        .split("// ---- puller dispatch ----")
        .nth(1)
        .and_then(|s| s.split("// ---- end puller dispatch ----").next())
        .expect("puller dispatch region");
    for kind in &names {
        assert_eq!(occurrences(arms, &format!("SourceKind::{} =>", variant_ident(kind))), 1, "puller arm: {kind}");
    }
    assert_eq!(occurrences(arms, "=>"), names.len(), "one arm per kind and no wildcard");
    assert!(!arms.contains("_ =>"));

    // gen-docs --check: clean tree passes; each hand-edited copy fails naming its file.
    let tmp = scratch_root("check");
    let clean = gen_docs_check(&tmp);
    assert!(clean.status.success(), "{}", String::from_utf8_lossy(&clean.stderr));
    for (file, kind) in [("docs/tools.md", "csv_url"), ("www/llms.txt", "airtable")] {
        let path = tmp.join(file);
        let text = std::fs::read_to_string(&path).unwrap();
        let edited = text.replacen(kind, &format!("{kind}_hand_edit"), 1);
        assert_ne!(text, edited, "{file} must carry {kind}");
        std::fs::write(&path, &edited).unwrap();
        let out = gen_docs_check(&tmp);
        assert_eq!(out.status.code(), Some(1), "{file}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(file), "{file} not named: {stderr}");
        std::fs::write(&path, text).unwrap();
    }
    let _ = std::fs::remove_dir_all(&tmp);
}
