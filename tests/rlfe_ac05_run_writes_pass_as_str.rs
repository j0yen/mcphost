//! contract: every INSERT INTO runs / UPDATE runs SET status in src/ binds its status and trigger through as_str()
//! PRD-mcphost-runs-list-filter-enums-generated
//! AC5 (P0) -- Given `src/` at the PR head, When the scanner collects every
//! quoted status or trigger literal inside an `INSERT INTO runs` or
//! `UPDATE runs SET status` statement, Then it finds none (every write passes
//! `as_str()`), and a planted `'stuck'` write in a fixture copy fails the
//! scanner naming file and line.

use std::path::{Path, PathBuf};

const STARTS: &[&str] = &["INSERT INTO runs", "UPDATE runs SET status"];

/// `(file, line, literal)` for every single-quoted literal in the written part
/// of an `INSERT INTO runs` / `UPDATE runs SET status` statement in `text`.
/// The written part is the statement's string literal up to a `WHERE` (reads
/// such as `WHERE status = 'running'` may stay literal).
fn scan(file: &str, text: &str) -> Vec<(String, usize, String)> {
    let mut out = Vec::new();
    for start in STARTS {
        for (at, _) in text.match_indices(start) {
            let rest = &text[at..];
            // The statement ends at the closing quote of its Rust string.
            let end = rest.find('"').unwrap_or(rest.len());
            let stmt = &rest[..end];
            let written = stmt.find(" WHERE ").map_or(stmt, |w| &stmt[..w]);
            let mut offset = 0;
            while let Some(open) = written[offset..].find('\'') {
                let open = offset + open;
                let Some(close) = written[open + 1..].find('\'') else { break };
                let literal = &written[open..open + close + 2];
                let line = text[..at + open].matches('\n').count() + 1;
                out.push((file.to_string(), line, literal.to_string()));
                offset = open + close + 2;
            }
        }
    }
    out.sort();
    out
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read src dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn rlfe_ac05() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rs_files(&root.join("src"), &mut files);
    assert!(files.len() > 50, "src enumeration looks broken: {}", files.len());
    let mut hits = Vec::new();
    let mut statements = 0usize;
    for f in &files {
        let text = std::fs::read_to_string(f).expect("read source");
        statements += STARTS.iter().map(|s| text.matches(s).count()).sum::<usize>();
        let rel = f.strip_prefix(root).unwrap().display().to_string();
        hits.extend(scan(&rel, &text));
    }
    assert!(statements >= 15, "scanner saw only {statements} run-write statements");
    assert!(hits.is_empty(), "quoted literal in a runs write (use RunStatus/RunTrigger::as_str()): {hits:?}");
}

#[test]
fn rlfe_ac05_planted_stuck_write_fails_naming_file_and_line() {
    let fixture = "fn f() {\n    conn.execute(\n        \"UPDATE runs SET status = 'stuck', error_class = ?1 \\\n         WHERE status = 'running'\",\n    );\n}\n";
    let hits = scan("fixture/db.rs", fixture);
    assert_eq!(hits, vec![("fixture/db.rs".to_string(), 3, "'stuck'".to_string())]);
    let insert = "x\n\"INSERT INTO runs (id, status) VALUES (?1, 'queued')\"";
    assert_eq!(scan("f.rs", insert), vec![("f.rs".to_string(), 2, "'queued'".to_string())]);
    let clean = "\"UPDATE runs SET status = ?1 WHERE status = 'running'\"";
    assert!(scan("f.rs", clean).is_empty());
}
