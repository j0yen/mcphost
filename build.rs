//! Stamps `MCPHOST_GIT_SHA` (12-char short sha) into the build when a `.git`
//! is present; leaves it unset otherwise so `build_info::BUILD.git_sha` is
//! `None`. Never fails the build when `git` is absent.
//!
//! Also embeds `examples/index.json` (PRD-mcphost-example-manifest-contract
//! R4): writes `$OUT_DIR/examples_embedded.rs`, which `src/examples.rs`
//! `include!`s as `EXAMPLES`.
use std::process::Command;

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

fn main() {
    embed_examples();
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
