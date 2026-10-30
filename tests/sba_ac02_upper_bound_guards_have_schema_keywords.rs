//! PRD-mcphost-schema-bound-agreement
//! contract: reads registry handler::host_tool_descriptors
//! AC2 (P0) -- Given `src/` at the PR head, When the generated scan reads every
//! `> CONST`/`>= CONST` on a `.len()` or numeric arg within `SCAN_WINDOW_LINES`
//! of a constructor in `INVALID_ARGS_CTORS`, Then each CONST is the value of a
//! descriptor `maxItems`/`maxLength`/`maximum` (a `*_BYTES` const only via a
//! `byte-bound` residue entry) or in `SCHEMA_VALID_REFUSALS`; else it fails
//! naming file:line.

use crate::vbd_calls;
use mcphost::host_args::{INVALID_ARGS_CTORS, SCAN_WINDOW_LINES, SCHEMA_VALID_REFUSALS};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).expect("read src dir").flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

/// Every `maxItems`/`maxLength`/`maximum` value anywhere in a descriptor schema.
fn bound_values(node: &Value, out: &mut BTreeSet<u64>) {
    match node {
        Value::Object(m) => {
            for (k, v) in m {
                if matches!(k.as_str(), "maxItems" | "maxLength" | "maximum")
                    && let Some(n) = v.as_u64()
                {
                    out.insert(n);
                }
                bound_values(v, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|v| bound_values(v, out)),
        _ => {}
    }
}

/// `const NAME: ty = <int | int * int>;` declarations, `(file, name, value)`.
fn const_decls(files: &[(PathBuf, Vec<String>)]) -> Vec<(PathBuf, String, u64)> {
    let mut out = Vec::new();
    for (path, lines) in files {
        for l in lines {
            let t = l.trim();
            let Some(rest) = t.strip_prefix("pub const ").or_else(|| t.strip_prefix("const ")).or_else(|| t.strip_prefix("pub(crate) const ")) else { continue };
            let Some((name, tail)) = rest.split_once(':') else { continue };
            let Some((_, expr)) = tail.split_once('=') else { continue };
            let expr = expr.trim().trim_end_matches(';');
            let value = expr.split('*').map(|f| f.trim().replace('_', "").parse::<u64>().ok()).try_fold(1u64, |acc, f| f.map(|f| acc * f));
            if let Some(v) = value {
                out.push((path.clone(), name.trim().to_string(), v));
            }
        }
    }
    out
}

/// The `CONST` after a `>` / `>=` on a line that reads a `.len()` or a numeric
/// arg (`as i64`/`as u64`/`as usize`, or an `i64`-typed local named `n`/`v`).
fn guard_consts(line: &str) -> Vec<String> {
    let t = line.trim();
    if t.starts_with("//") || !(t.contains(".len()") || t.contains(" as i64") || t.contains(" as u64") || t.contains(" as usize")) {
        return Vec::new();
    }
    let mut out = Vec::new();
    let bytes = t.as_bytes();
    for (i, _) in t.match_indices('>') {
        if i > 0 && matches!(bytes[i - 1], b'-' | b'=') {
            continue; // `->` / `=>`
        }
        let mut rest = &t[i + 1..];
        rest = rest.strip_prefix('=').unwrap_or(rest).trim_start();
        let ident: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == ':').collect();
        let last = ident.rsplit("::").next().unwrap_or("");
        if last.len() > 2 && last.chars().next().is_some_and(|c| c.is_ascii_uppercase()) && last.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') {
            out.push(last.to_string());
        }
    }
    out
}

#[test]
fn sba_ac02_every_upper_bound_guard_maps_to_a_schema_keyword_or_residue() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut paths = Vec::new();
    rust_files(&root, &mut paths);
    let files: Vec<(PathBuf, Vec<String>)> = paths
        .into_iter()
        .map(|p| {
            let text = std::fs::read_to_string(&p).expect("read source");
            (p, text.lines().map(str::to_string).collect())
        })
        .collect();
    let decls = const_decls(&files);

    let mut values = BTreeSet::new();
    for tool in vbd_calls::all_descriptors() {
        bound_values(&Value::Object((*tool.input_schema).clone()), &mut values);
    }

    let mut scanned = 0;
    let mut failures = Vec::new();
    for (path, lines) in &files {
        for (i, line) in lines.iter().enumerate() {
            for konst in guard_consts(line) {
                let lo = i.saturating_sub(SCAN_WINDOW_LINES);
                let hi = (i + SCAN_WINDOW_LINES + 1).min(lines.len());
                if !lines[lo..hi].iter().any(|l| INVALID_ARGS_CTORS.iter().any(|c| l.contains(c))) {
                    continue;
                }
                scanned += 1;
                let at = format!("{}:{}", path.strip_prefix(env!("CARGO_MANIFEST_DIR")).unwrap().display(), i + 1);
                let mine: Vec<u64> = decls.iter().filter(|(p, n, _)| n == &konst && p == path).map(|d| d.2).collect();
                let anywhere: Vec<u64> = decls.iter().filter(|(_, n, _)| n == &konst).map(|d| d.2).collect();
                let value = mine.first().or(anywhere.first()).copied();
                let residue = SCHEMA_VALID_REFUSALS.iter().any(|r| r.konst == konst);
                let byte_residue = SCHEMA_VALID_REFUSALS.iter().any(|r| r.konst == konst && r.reason == "byte-bound");
                let ok = if konst.ends_with("_BYTES") {
                    byte_residue
                } else {
                    residue || value.is_some_and(|v| values.contains(&v))
                };
                if !ok {
                    failures.push(format!("{at}: `{konst}` ({}) is not a descriptor maxItems/maxLength/maximum and has no SCHEMA_VALID_REFUSALS entry", value.map_or("unresolved".into(), |v| v.to_string())));
                }
            }
        }
    }
    println!("sba_ac02: {scanned} guards scanned, {} failures", failures.len());
    assert!(scanned >= 10, "the scan found only {scanned} guards; the window or patterns drifted");
    assert!(failures.is_empty(), "{} upper-bound guards without a schema keyword:\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn sba_ac02_guard_pattern_reads_len_and_numeric_comparisons() {
    assert_eq!(guard_consts("if xs.len() > crate::foo::MAX_X {"), vec!["MAX_X".to_string()]);
    assert_eq!(guard_consts("if n as i64 >= LIMIT_N {"), vec!["LIMIT_N".to_string()]);
    assert!(guard_consts("let f = |x| -> Result<usize, E> { xs.len() };").is_empty());
    assert!(guard_consts("// if xs.len() > MAX_X {").is_empty());
}
