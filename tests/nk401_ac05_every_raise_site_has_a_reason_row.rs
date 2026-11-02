//! PRD-mcphost-named-key-refusal-not-http-401
//! AC5 -- every `TenantKeyMissing(` construction in `src/` constructs an
//! explicit `Reason::` variant and has a row in the decision table; a
//! planted site with no row fails with file:line; `TenantKeyMissing(Some(`
//! and `TenantKeyMissing(None` match zero lines.

use mcphost::errors::Reason;
use mcphost::http::{PathClass, Upgrade, upgrade_for};
use std::path::{Path, PathBuf};

/// The decision table's raise-site rows: (file, Reason variant, bare-path
/// upgrade). A construction site is only allowed if it appears here.
const SITES: &[(&str, &str, Upgrade)] = &[
    ("src/handler.rs", "ConnectionBound", Upgrade::None),
    ("src/handler.rs", "PeerRecent", Upgrade::None),
    ("src/handler.rs", "Anonymous", Upgrade::To401),
    ("src/errors.rs", "Anonymous", Upgrade::To401),
];

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Is the text after `TenantKeyMissing(` a pattern (`_` or a lowercase
/// binding, or the declaration) rather than a constructor argument?
fn is_pattern(rest: &str) -> bool {
    // The variant's own declaration, `TenantKeyMissing(Reason),`.
    if rest.starts_with("Reason)") {
        return true;
    }
    let arg: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    arg == "_" || (!arg.is_empty() && arg.chars().next().is_some_and(|c| c.is_ascii_lowercase()))
}

fn variant_of(rest: &str) -> Option<&str> {
    let rest = rest.strip_prefix("Reason::").or_else(|| rest.strip_prefix("crate::errors::Reason::"))?;
    let end = rest.find(|c: char| !c.is_ascii_alphanumeric()).unwrap_or(rest.len());
    Some(&rest[..end])
}

/// Scan `(path, text)` pairs; returns one `file:line: why` per violation.
fn scan(files: &[(String, String)]) -> Vec<String> {
    let mut bad = vec![];
    for (path, text) in files {
        for (i, line) in text.lines().enumerate() {
            let mut from = 0;
            while let Some(at) = line[from..].find("TenantKeyMissing(") {
                let rest = &line[from + at + "TenantKeyMissing(".len()..];
                from += at + 1;
                if is_pattern(rest) {
                    continue;
                }
                let loc = format!("{path}:{}", i + 1);
                match variant_of(rest) {
                    None => bad.push(format!("{loc}: no explicit Reason:: variant")),
                    Some(v) if !SITES.iter().any(|(f, sv, _)| path.ends_with(f) && *sv == v) => {
                        bad.push(format!("{loc}: Reason::{v} has no decision-table row"))
                    }
                    Some(_) => {}
                }
            }
        }
    }
    bad
}

fn read_src() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut paths = vec![];
    rs_files(&root.join("src"), &mut paths);
    paths
        .into_iter()
        .map(|p| {
            let rel = p.strip_prefix(root).expect("under root").to_string_lossy().into_owned();
            (rel, std::fs::read_to_string(&p).expect("read src file"))
        })
        .collect()
}

#[test]
fn nk401_ac05() {
    let files = read_src();
    assert_eq!(scan(&files), Vec::<String>::new());

    // Every table row matches what the decision function does on bare /mcp.
    for (file, variant, want) in SITES {
        let reason = match *variant {
            "ConnectionBound" => Reason::ConnectionBound("ns".into()),
            "PeerRecent" => Reason::PeerRecent,
            _ => Reason::Anonymous,
        };
        assert_eq!(upgrade_for(PathClass::Bare, "tenant_key_missing", &reason), *want, "{file} {variant}");
    }

    // Each table row is actually used by a site (no stale rows).
    for (file, variant, _) in SITES {
        let needle = "TenantKeyMissing(".to_string();
        let used = files.iter().filter(|(p, _)| p.ends_with(file)).any(|(_, t)| {
            t.lines().any(|l| l.contains(&needle) && l.contains(&format!("Reason::{variant}")))
        });
        assert!(used, "stale decision-table row {file} {variant}");
    }

    // The removed Option forms never come back.
    for (path, text) in &files {
        for (i, line) in text.lines().enumerate() {
            assert!(
                !line.contains("TenantKeyMissing(Some(") && !line.contains("TenantKeyMissing(None"),
                "{path}:{}: Option form of TenantKeyMissing",
                i + 1
            );
        }
    }
}

#[test]
fn nk401_ac05_planted_site_fails_with_file_line() {
    let planted = vec![(
        "src/planted.rs".to_string(),
        "fn a() {}\nlet e = AppError::TenantKeyMissing(Reason::PeerRecent);\nlet f = AppError::TenantKeyMissing(x);\n"
            .to_string(),
    )];
    let bad = scan(&planted);
    assert_eq!(bad, vec!["src/planted.rs:2: Reason::PeerRecent has no decision-table row".to_string()]);
    let bare = vec![("src/handler.rs".to_string(), "AppError::TenantKeyMissing(Default::default())".to_string())];
    assert_eq!(scan(&bare), vec!["src/handler.rs:1: no explicit Reason:: variant".to_string()]);
}
