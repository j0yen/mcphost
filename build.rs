//! Stamps `MCPHOST_GIT_SHA` (12-char short sha) into the build when a `.git`
//! is present; leaves it unset otherwise so `build_info::BUILD.git_sha` is
//! `None`. Never fails the build when `git` is absent.
//!
//! Also embeds `examples/index.json` (PRD-mcphost-example-manifest-contract
//! R4): writes `$OUT_DIR/examples_embedded.rs`, which `src/examples.rs`
//! `include!`s as `EXAMPLES`.
use std::process::Command;

#[path = "src/doc_scan.rs"]
mod doc_scan;

/// Set to let a build proceed without a usable `examples/index.json` (the
/// generator `mcphost examples index` has to be built before it can run).
const REGEN_ENV: &str = "MCPHOST_EXAMPLES_REGEN";

fn str_list(v: &serde_json::Value) -> String {
    let items: Vec<String> = v
        .as_array()
        .map(|a| a.iter().map(|x| format!("{:?}", x.as_str().unwrap_or_default())).collect())
        .unwrap_or_default();
    format!("&[{}]", items.join(", "))
}

fn s(v: &serde_json::Value, key: &str) -> String {
    format!("{:?}", v[key].as_str().unwrap_or_default())
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// R4: the first thing found out of step with `index.json` (a manifest or
/// tool file whose hash changed, an example directory the index lacks, an
/// index entry whose directory is gone), named by its repo-relative path.
fn stale_reason(root: &std::path::Path, index: &serde_json::Value) -> Option<String> {
    let entries = index["examples"].as_array().cloned().unwrap_or_default();
    let mut indexed = Vec::new();
    for ex in &entries {
        let name = ex["name"].as_str().unwrap_or_default();
        indexed.push(name.to_string());
        let dir = root.join(name);
        let drifted = |rel: &str, want: &serde_json::Value| -> Option<String> {
            let got = std::fs::read(dir.join(rel)).ok().map(|b| sha256_hex(&b));
            (got.as_deref() != want.as_str()).then(|| format!("examples/{name}/{rel}"))
        };
        if let Some(f) = drifted("example.toml", &ex["manifest_sha256"]) {
            return Some(f);
        }
        for t in ex["tools"].as_array().cloned().unwrap_or_default() {
            if let Some(f) = drifted(t["file"].as_str().unwrap_or_default(), &t["sha256"]) {
                return Some(f);
            }
        }
    }
    let mut dirs: Vec<String> = std::fs::read_dir(root)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    dirs.sort();
    dirs.into_iter().find(|d| !indexed.contains(d)).map(|d| format!("examples/{d}"))
}

fn embed_examples() {
    println!("cargo:rerun-if-changed=examples");
    println!("cargo:rerun-if-env-changed={REGEN_ENV}");
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let root = std::path::Path::new(&manifest_dir).join("examples");
    let regen = std::env::var_os(REGEN_ENV).is_some();
    let index: serde_json::Value = match std::fs::read_to_string(root.join("index.json")) {
        Ok(text) => serde_json::from_str(&text).expect("examples/index.json is not valid JSON"),
        Err(_) if regen => serde_json::json!({ "examples": [] }),
        Err(_) => panic!("examples/index.json is missing; run mcphost examples index"),
    };
    if !regen && let Some(path) = stale_reason(&root, &index) {
        panic!("examples/index.json is stale; run mcphost examples index ({path} is out of step with it)");
    }
    let mut out = String::from("pub static EXAMPLES: &[Example] = &[\n");
    for ex in index["examples"].as_array().cloned().unwrap_or_default() {
        let name = ex["name"].as_str().unwrap_or_default();
        let mut tools = String::new();
        for t in ex["tools"].as_array().cloned().unwrap_or_default() {
            let file = t["file"].as_str().unwrap_or_default();
            let path = root.join(name).join(file);
            let network = match t["network"].as_str() {
                Some(n) => format!("Some({n:?})"),
                None => "None".to_string(),
            };
            tools.push_str(&format!(
                "ExampleTool {{ name: {}, kind: {}, file: {}, source: include_str!({:?}), network: {network} }},\n",
                s(&t, "name"),
                s(&t, "kind"),
                s(&t, "file"),
                path.to_string_lossy()
            ));
        }
        let fc = &ex["first_call"];
        out.push_str(&format!(
            "Example {{ name: {}, title: {}, summary: {}, use_case: {}, clients: {}, since: {}, proof: {}, secrets_needed: {}, tools: &[{tools}], first_call: ExampleFirstCall {{ tool: {}, args_json: {:?}, expect: {} }} }},\n",
            s(&ex, "name"),
            s(&ex, "title"),
            s(&ex, "summary"),
            str_list(&ex["use_case"]),
            str_list(&ex["clients"]),
            s(&ex, "since"),
            s(&ex, "proof"),
            str_list(&ex["secrets_needed"]),
            s(fc, "tool"),
            fc["args"].to_string(),
            s(fc, "expect"),
        ));
    }
    out.push_str("];\n");
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
    std::fs::write(std::path::Path::new(&out_dir).join("examples_embedded.rs"), out).expect("write examples_embedded.rs");
}

/// PRD-mcphost-migration-refusal-contract R3: the migration index the
/// compat report reads is GENERATED from `migrations/*.sql` (name, the
/// first line's `-- compat:` header, and the file text), never listed by
/// hand, so a new migration file cannot be missing from the report.
/// A file whose header is neither word is indexed as `breaking` (the
/// fail-safe reading); `tests/migration_compat_headers.rs` keeps that
/// case from ever landing.
fn embed_migrations_index() {
    println!("cargo:rerun-if-changed=migrations");
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let root = std::path::Path::new(&manifest_dir).join("migrations");
    let mut names: Vec<String> = std::fs::read_dir(&root)
        .expect("read migrations/")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "sql"))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let mut out = String::from("pub static MIGRATION_INDEX: &[(&str, &str, &str)] = &[\n");
    for name in &names {
        let path = root.join(name);
        let text = std::fs::read_to_string(&path).expect("read migration");
        let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim_start();
        let compat = if first.starts_with("-- compat: previous") { "previous" } else { "breaking" };
        out.push_str(&format!("({name:?}, {compat:?}, include_str!({:?})),\n", path.to_string_lossy()));
    }
    out.push_str("];\n");
    let dest = std::path::Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("migrations_index.rs");
    std::fs::write(dest, out).expect("write migrations_index.rs");
}

fn main() {
    embed_examples();
    embed_help_docs();
    embed_doc_anchors();
    embed_migrations_index();
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-env-changed=MCPHOST_GIT_SHA");
    if std::env::var_os("MCPHOST_GIT_SHA").is_some() {
        return;
    }
    if !std::path::Path::new(".git").exists() {
        return;
    }
    let Ok(out) = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
    else {
        return;
    };
    if !out.status.success() {
        return;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !sha.is_empty() {
        println!("cargo:rustc-env=MCPHOST_GIT_SHA={sha}");
    }
}

/// PRD-mcphost-help-search-tool R1: writes `$OUT_DIR/help_docs_embedded.rs`,
/// which `src/help_search.rs` `include!`s as `HELP_DOCS` -- README.md, every
/// `docs/**/*.md` found on disk (so a new page is indexed without a code
/// edit) and `www/llms.txt`, each as `(repo-relative path, include_str!)`.
fn embed_help_docs() {
    println!("cargo:rerun-if-changed=docs");
    println!("cargo:rerun-if-changed=README.md");
    println!("cargo:rerun-if-changed=www/llms.txt");
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let root = std::path::Path::new(&manifest_dir);
    let mut paths = vec!["README.md".to_string(), "www/llms.txt".to_string()];
    let mut stack = vec![root.join("docs")];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for entry in rd.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "md")
                && let Ok(rel) = path.strip_prefix(root)
            {
                paths.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    paths.sort();
    let mut out = String::from("pub static HELP_DOCS: &[(&str, &str)] = &[\n");
    for rel in &paths {
        out.push_str(&format!("({rel:?}, include_str!({:?})),\n", root.join(rel).to_string_lossy()));
    }
    out.push_str("];\n");
    let dest = std::path::Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("help_docs_embedded.rs");
    std::fs::write(dest, out).expect("write help_docs_embedded.rs");
}

/// CamelCase variant name from `<path>#<slug>`: ASCII alphanumeric runs,
/// each capitalised, `.md`/`.txt` dropped from the path.
fn variant_name(path: &str, slug: &str) -> String {
    let stem = path.trim_end_matches(".md").trim_end_matches(".txt");
    let mut out = String::new();
    for word in stem.split(|c: char| !c.is_ascii_alphanumeric()).chain(slug.split(|c: char| !c.is_ascii_alphanumeric())) {
        let mut cs = word.chars();
        if let Some(first) = cs.next() {
            out.push(first.to_ascii_uppercase());
            out.extend(cs.map(|c| c.to_ascii_lowercase()));
        }
    }
    out
}

/// PRD-mcphost-emitted-doc-pointers-resolve R1: writes
/// `$OUT_DIR/doc_anchors.rs`, which `src/doc_anchors.rs` `include!`s -- one
/// `DocAnchor` variant and one `ENTRIES` row per markdown heading of
/// README.md, `docs/**/*.md`, `www/llms.txt` and `www/llms-install.md`.
fn embed_doc_anchors() {
    println!("cargo:rerun-if-changed=docs");
    println!("cargo:rerun-if-changed=README.md");
    println!("cargo:rerun-if-changed=www/llms.txt");
    println!("cargo:rerun-if-changed=www/llms-install.md");
    println!("cargo:rerun-if-changed=src/doc_scan.rs");
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let root = std::path::Path::new(&manifest_dir);
    let mut paths = vec!["README.md".to_string(), "www/llms.txt".to_string(), "www/llms-install.md".to_string()];
    let mut stack = vec![root.join("docs")];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for entry in rd.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "md")
                && let Ok(rel) = path.strip_prefix(root)
            {
                paths.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    paths.sort();
    let mut names: Vec<String> = Vec::new();
    let mut rows = String::new();
    for rel in &paths {
        let Ok(text) = std::fs::read_to_string(root.join(rel)) else { continue };
        for h in doc_scan::scan(&text) {
            let mut name = variant_name(rel, &h.slug);
            let base = name.clone();
            let mut n = 2;
            while names.contains(&name) {
                name = format!("{base}{n}");
                n += 1;
            }
            let example = match &h.example {
                Some((lang, code)) => format!("Some(({lang:?}, {code:?}))"),
                None => "None".to_string(),
            };
            rows.push_str(&format!(
                "AnchorEntry {{ anchor: {:?}, path: {rel:?}, title: {:?}, level: {}, line: {}, text: {:?}, example: {example} }},\n",
                format!("{rel}#{}", h.slug),
                h.title,
                h.level,
                h.line,
                h.text,
            ));
            names.push(name);
        }
    }
    let mut out = String::from("#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]\npub enum DocAnchor {\n");
    for name in &names {
        out.push_str(&format!("    {name},\n"));
    }
    out.push_str("}\n\npub const ALL: &[DocAnchor] = &[\n");
    for name in &names {
        out.push_str(&format!("    DocAnchor::{name},\n"));
    }
    out.push_str("];\n\npub static ENTRIES: &[AnchorEntry] = &[\n");
    out.push_str(&rows);
    out.push_str("];\n");
    let dest = std::path::Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("doc_anchors.rs");
    std::fs::write(dest, out).expect("write doc_anchors.rs");
}
