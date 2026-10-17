//! PRD-mcphost-errors-json-contract
//! AC1 (P0) — Given `src/**/*.rs` at the PR head, When the scan test collects
//! every `code: "…"`, `structured("…")`, `structured_with("…")`,
//! `"error_code": "…"` literal and every `=> "…"` arm of `AppError::code()`,
//! Then every collected code is a row of `ERROR_CODES` and the test names any
//! code built by `format!` as a failure, not a skip.

use mcphost::error_registry::{ERROR_CODES, ErrorClass, lookup};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read_dir").filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

const PREFIXES: &[&str] = &["code:", "structured(", "structured_with(", "\"error_code\":"];

/// What follows a prefix: a string literal (the code), a `format!` (a
/// failure), or anything else (a variable pass-through, not a literal).
enum Hit {
    Literal(String),
    Format,
}

fn hits_after_prefix(text: &str, prefix: &str) -> Vec<Hit> {
    let mut hits = Vec::new();
    let mut from = 0;
    while let Some(i) = text[from..].find(prefix) {
        let at = from + i + prefix.len();
        from = at;
        let rest = text[at..].trim_start();
        if let Some(lit) = rest.strip_prefix('"') {
            if let Some(end) = lit.find('"') {
                hits.push(Hit::Literal(lit[..end].to_string()));
            }
        } else if rest.starts_with("format!") {
            hits.push(Hit::Format);
        }
    }
    hits
}

/// The `=> "…"` arms inside `AppError::code()`'s body.
fn code_arms(errors_rs: &str) -> Vec<String> {
    let start = errors_rs.find("pub fn code(&self)").expect("AppError::code");
    let body = &errors_rs[start..];
    let end = body.find("fn jsonrpc_code").expect("jsonrpc_code follows code()");
    let body = &body[..end];
    let mut arms = Vec::new();
    let mut from = 0;
    while let Some(i) = body[from..].find("=>") {
        let at = from + i + 2;
        from = at;
        if let Some(lit) = body[at..].trim_start().strip_prefix('"')
            && let Some(end) = lit.find('"')
        {
            arms.push(lit[..end].to_string());
        }
    }
    arms
}

fn well_formed(code: &str) -> bool {
    !code.is_empty() && code.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

#[test]
fn errjson_ac01_every_emitted_code_is_a_registry_row() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    assert!(files.len() > 50, "scan found only {} files", files.len());
    files.sort();

    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    let mut format_built = Vec::new();
    for file in &files {
        let rel = file.strip_prefix(root).unwrap().display().to_string();
        let text = std::fs::read_to_string(file).expect("read");
        for prefix in PREFIXES {
            for hit in hits_after_prefix(&text, prefix) {
                match hit {
                    Hit::Format => format_built.push(format!("{rel}: `{prefix}` followed by format!")),
                    // `error_code: ""` is the dead-letter table's "no code".
                    Hit::Literal(code) if code.is_empty() => {}
                    Hit::Literal(code) if !well_formed(&code) => {
                        format_built.push(format!("{rel}: `{prefix}` literal {code:?} is not a static code"));
                    }
                    Hit::Literal(code) => {
                        seen.entry(code).or_insert_with(|| rel.clone());
                    }
                }
            }
        }
        if rel == "src/errors.rs" {
            for arm in code_arms(&text) {
                assert!(well_formed(&arm), "AppError::code() arm {arm:?} is not a static code");
                seen.entry(arm).or_insert_with(|| rel.clone());
            }
        }
    }

    assert!(
        format_built.is_empty(),
        "codes built at runtime cannot be enumerated; use a static literal and a registry row:\n{}",
        format_built.join("\n")
    );
    assert!(seen.len() > 150, "scan collected only {} codes; the patterns drifted", seen.len());
    let missing: Vec<String> = seen
        .iter()
        .filter(|(code, _)| lookup(code).is_none())
        .map(|(code, file)| format!("{code} (first seen in {file})"))
        .collect();
    assert!(
        missing.is_empty(),
        "codes emitted in src/ with no row in ERROR_CODES (add one in src/error_registry.rs):\n{}",
        missing.join("\n")
    );
}

#[test]
fn errjson_ac01_code_arms_are_found_by_parsing_errors_rs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(root.join("src/errors.rs")).expect("read errors.rs");
    let arms = code_arms(&text);
    assert!(arms.len() >= 35, "only {} arms parsed from AppError::code()", arms.len());
    for known in ["bearer_invalid", "tool_not_found", "quota_trusted_issuers", "args_invalid"] {
        assert!(arms.iter().any(|a| a == known), "arm {known} not parsed");
        assert!(lookup(known).is_some(), "arm {known} has no registry row");
    }
}

#[test]
fn errjson_ac01_every_code_in_the_compiled_enums_is_a_registry_row() {
    for variant in mcphost::errors::all_variants_for_test() {
        let code = variant.code();
        assert!(lookup(code).is_some(), "AppError variant code {code} has no registry row");
    }
    for code in mcphost::errors::HOST_FAULT_CODES.iter().chain(mcphost::feedback::KIND_LEVEL_CODES) {
        assert!(lookup(code).is_some(), "{code} has no registry row");
    }
    for code in [
        mcphost::kinds::contract::codes::TOOL_EXCEPTION,
        mcphost::kinds::contract::codes::TOOL_TRAPPED,
        mcphost::kinds::contract::codes::TOOL_OUTPUT_INVALID,
        mcphost::kinds::contract::codes::TOOL_TIMEOUT,
        mcphost::kinds::contract::codes::TOOL_OOM,
        mcphost::kinds::contract::codes::TOOL_PROCESS_LIMIT,
        mcphost::kinds::contract::codes::TOOL_OUTPUT_TOO_LARGE,
        mcphost::kinds::contract::codes::TOOL_BUILDING,
        mcphost::kinds::contract::codes::UPSTREAM_AUTH,
        mcphost::kinds::contract::codes::UPSTREAM_RATE_LIMITED,
        mcphost::kinds::contract::codes::COMPOSE_MAPPING_MISSING,
    ] {
        assert!(lookup(code).is_some(), "{code} has no registry row");
    }
}

#[test]
fn errjson_ac01_registry_is_sorted_unique_and_well_formed() {
    let codes: Vec<&str> = ERROR_CODES.iter().map(|e| e.code).collect();
    let mut sorted = codes.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(codes, sorted, "ERROR_CODES must be sorted by code with no duplicates");
    for e in ERROR_CODES {
        assert!(well_formed(e.code), "{}", e.code);
        assert!(!e.remedy.is_empty() && e.remedy.len() <= 200, "{}: remedy length {}", e.code, e.remedy.len());
    }
    for class in ErrorClass::ALL {
        assert!(ERROR_CODES.iter().any(|e| e.class == *class), "no code has class {}", class.as_str());
    }
}
