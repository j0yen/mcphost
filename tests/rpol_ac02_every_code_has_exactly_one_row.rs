//! PRD-mcphost-retry-policy-table-generated AC2 (R3) — Given the host, When
//! the completeness test runs, Then every `AppError::code()` value and every
//! `KindError::structured` literal has exactly one `RETRY_POLICY` row and no
//! row names an unknown code.
//!
//! "Emittable" codes come from four places, all derived here, none copied:
//! the `AppError` variants (`all_variants_for_test`), the kind-level codes,
//! the first argument of every `KindError::structured(_with)(` call in
//! `src/` (a literal, a `codes::NAME` resolved against
//! `src/kinds/contract.rs`, or a path ending in a const declared by an
//! `err_code!` block anywhere in `src/`; any other first argument fails with
//! file:line unless `COMPUTED` below names the set it draws from), and the handler-level
//! `AppError::Structured { code: ... }` literals, which only widen the set of
//! codes a row may legitimately name.

use mcphost::retry_policy::RETRY_POLICY;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read src dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn consts(contract_src: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in contract_src.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("pub const ")
            && let Some((name, rest)) = rest.split_once(": &str = \"")
            && let Some((value, _)) = rest.split_once('"')
        {
            map.insert(name.to_string(), value.to_string());
        }
    }
    map
}

/// Every `err_code!` declaration in `src_text`: const name -> code, for both
/// the `pub NAME, code: "x"` and the `pub NAME => "x"` forms.
fn err_code_consts(src_text: &str, map: &mut BTreeMap<String, String>) {
    for (at, _) in src_text.match_indices("err_code! {") {
        let mut rest = src_text[at + "err_code! {".len()..].trim_start();
        while rest.starts_with("#[") || rest.starts_with("///") {
            rest = rest.split_once('\n').map_or("", |(_, r)| r).trim_start();
        }
        let rest = rest.strip_prefix("pub ").unwrap_or(rest);
        let name_end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(rest.len());
        let (name, rest) = rest.split_at(name_end);
        let rest = rest.trim_start();
        let rest = rest
            .strip_prefix(',')
            .map(|r| r.trim_start().strip_prefix("code:").unwrap_or(r))
            .or_else(|| rest.strip_prefix("=>"));
        if let Some(rest) = rest
            && let Some(rest) = rest.trim_start().strip_prefix('"')
            && let Some((value, _)) = rest.split_once('"')
            && !name.is_empty()
        {
            map.insert(name.to_string(), value.to_string());
        }
    }
}

/// First argument of a call whose opening paren is at `open`, trimmed.
fn first_arg(src: &str, open: usize) -> &str {
    let rest = &src[open..];
    let end = rest.find([',', ')']).unwrap_or(rest.len());
    rest[..end].trim()
}

fn line_of(src: &str, at: usize) -> usize {
    src[..at].matches('\n').count() + 1
}

/// Non-literal first arguments, each with the set of codes it can carry.
fn computed() -> Vec<(&'static str, &'static str, Vec<&'static str>)> {
    vec![
        // `remedy.code` over `UPSTREAM_REMEDIES`.
        ("src/kinds/http.rs", "remedy.code", mcphost::kinds::http::UPSTREAM_REMEDIES.iter().map(|r| r.code).filter(|c| !c.contains('.')).collect()),
        // a `McphostElicitError` message prefix drawn from this list.
        ("src/kinds/python.rs", "code", mcphost::elicit::ELICIT_ERROR_CODES.to_vec()),
        // a `McphostTableError` message prefix drawn from this list.
        ("src/kinds/python.rs", "table_code", mcphost::tables::TABLE_ERROR_CODES.to_vec()),
        // the chain step wrapper's `("exec", ..)` arm; its other arms are literals.
        ("src/kinds/chain.rs", "code", vec!["exec"]),
    ]
}

#[test]
fn rpol_ac02() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rs_files(&root.join("src"), &mut files);
    files.sort();
    let consts = consts(&std::fs::read_to_string(root.join("src/kinds/contract.rs")).expect("contract.rs"));
    let mut declared = BTreeMap::new();
    for path in &files {
        err_code_consts(&std::fs::read_to_string(path).expect("read src file"), &mut declared);
    }
    let computed = computed();

    let mut emittable: BTreeSet<String> = BTreeSet::new();
    let mut known: BTreeSet<String> = BTreeSet::new();
    let mut unresolved = Vec::new();

    for v in mcphost::errors::all_variants_for_test() {
        emittable.insert(v.code().to_string());
    }
    for c in mcphost::feedback::KIND_LEVEL_CODES {
        emittable.insert((*c).to_string());
    }

    for path in &files {
        let rel = path.strip_prefix(&root).expect("under root").to_string_lossy().to_string();
        let src = std::fs::read_to_string(path).expect("read src file");
        for needle in ["KindError::structured(", "KindError::structured_with("] {
            for (at, _) in src.match_indices(needle) {
                let arg = first_arg(&src, at + needle.len());
                if let Some(lit) = arg.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
                    emittable.insert(lit.to_string());
                } else if let Some(value) = arg.strip_prefix("codes::").and_then(|n| consts.get(n)) {
                    emittable.insert(value.clone());
                } else if let Some(value) = arg.rsplit("::").next().and_then(|n| declared.get(n)) {
                    emittable.insert(value.clone());
                } else if let Some((_, _, set)) = computed.iter().find(|(f, a, _)| *f == rel && *a == arg) {
                    emittable.extend(set.iter().map(|s| s.to_string()));
                } else {
                    unresolved.push(format!("{rel}:{} first argument `{arg}` is not a literal", line_of(&src, at)));
                }
            }
        }
        for (at, _) in src.match_indices("Structured {") {
            let rest = src[at + "Structured {".len()..].trim_start();
            if let Some(rest) = rest.strip_prefix("code:") {
                let arg = rest.trim_start();
                let end = arg.find([',', '}']).unwrap_or(arg.len());
                let arg = arg[..end].trim();
                if let Some(lit) = arg.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
                    known.insert(lit.to_string());
                } else if let Some(value) = arg.strip_prefix("codes::").and_then(|n| consts.get(n)) {
                    known.insert(value.clone());
                }
            }
        }
    }
    assert!(unresolved.is_empty(), "codes hidden behind variables:\n{}", unresolved.join("\n"));
    known.extend(emittable.iter().cloned());
    // every `err_code!`-declared code is a registered code a row may name.
    known.extend(declared.values().cloned());
    assert!(emittable.len() > 80, "the scan found only {} codes; the scanner is broken", emittable.len());

    let mut rows: BTreeMap<&str, usize> = BTreeMap::new();
    for r in RETRY_POLICY {
        *rows.entry(r.code).or_default() += 1;
    }
    let duplicated: Vec<_> = rows.iter().filter(|(_, n)| **n > 1).map(|(c, _)| *c).collect();
    assert!(duplicated.is_empty(), "codes with more than one RETRY_POLICY row: {duplicated:?}");
    let missing: Vec<_> = emittable.iter().filter(|c| !rows.contains_key(c.as_str())).collect();
    assert!(missing.is_empty(), "emittable codes with no RETRY_POLICY row: {missing:?}");
    let unknown: Vec<_> = rows.keys().filter(|c| !known.contains(**c)).collect();
    assert!(unknown.is_empty(), "RETRY_POLICY rows naming a code nothing emits: {unknown:?}");
}
