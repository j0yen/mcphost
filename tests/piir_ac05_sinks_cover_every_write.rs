//! PRD-mcphost-pii-tagging-and-log-redaction-v2
//! AC5 -- Given the source tree, When `piir_sinks_cover_every_write` runs, Then
//! every write into a sink table or column is in a function named in the
//! `LogSink` table or in an allowlist entry with a reason; Given a planted
//! `INSERT INTO logs` outside both, Then the test fails naming the file and
//! line.

use mcphost::logsink::{ALLOWLIST, LogSink};
use regex::Regex;
use std::path::{Path, PathBuf};

/// One write into a sink table or column found in a source file.
#[derive(Debug)]
struct Write {
    file: String,
    line: usize,
    function: String,
    what: String,
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read_dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn line_of(text: &str, offset: usize) -> usize {
    text[..offset].matches('\n').count() + 1
}

/// The name of the function whose body holds `line` (1-based): the nearest
/// `fn` at or above it.
fn enclosing_fn(lines: &[&str], line: usize) -> String {
    let re = Regex::new(r"^\s*(?:pub(?:\([a-z]+\))?\s+)?(?:async\s+)?(?:unsafe\s+)?fn\s+(\w+)").expect("regex");
    lines[..line]
        .iter()
        .rev()
        .find_map(|l| re.captures(l).map(|c| c[1].to_string()))
        .unwrap_or_else(|| "<outside any fn>".to_string())
}

/// The text of the function `name` (from its `fn` line to the closing brace at
/// the same indentation), or `None` when the file has no such function.
fn fn_body(text: &str, name: &str) -> Option<String> {
    let re = Regex::new(&format!(r"(?m)^(\s*)(?:pub(?:\([a-z]+\))?\s+)?(?:async\s+)?fn\s+{name}\b")).expect("regex");
    let caps = re.captures(text)?;
    let start = caps.get(0)?.start();
    let indent = caps[1].to_string();
    let rest = &text[start..];
    let close = format!("\n{indent}}}\n");
    let end = rest.find(&close).map_or(rest.len(), |i| i + close.len());
    Some(rest[..end].to_string())
}

/// Every write into a sink table or column in `files` (`(repo-relative path, text)`).
fn find_writes(files: &[(String, String)]) -> Vec<Write> {
    // The table a statement writes, then what must hold for it to be a sink write.
    let stmt = Regex::new(r"(?i)(INSERT\s+(?:OR\s+\w+\s+)?INTO|UPDATE)\s+(logs|messages|call_traces|calls|runs)\b").expect("regex");
    let mut writes = Vec::new();
    for (file, text) in files {
        let lines: Vec<&str> = text.lines().collect();
        for m in stmt.captures_iter(text) {
            let verb = m[1].to_ascii_uppercase();
            let table = m[2].to_ascii_lowercase();
            let start = m.get(0).expect("match").start();
            let window: String = text[start..].chars().take(1500).collect();
            // An INSERT's column list ends at VALUES (or SELECT); an UPDATE's SET list at WHERE.
            let limit = if verb.starts_with("INSERT") { ["VALUES", "SELECT"] } else { ["WHERE", "WHERE"] };
            let upper = window.to_ascii_uppercase();
            let cut = limit.iter().filter_map(|l| upper.find(l)).min().unwrap_or(window.len());
            let head = &window[..cut];
            // The sinks: the three tables take text only by INSERT (an UPDATE of a
            // dedupe key or a test back-date writes no tool output), the two
            // columns are checked by name.
            let is_sink_write = match (verb.starts_with("INSERT"), table.as_str()) {
                (true, "logs" | "messages" | "call_traces") => true,
                (true, "calls") => head.contains("error_summary"),
                (_, "runs") => head.contains("error_data_json"),
                _ => false,
            };
            if !is_sink_write || file.starts_with("src/logsink.rs") {
                continue;
            }
            let line = line_of(text, start);
            // A doc or line comment that quotes the statement is not a write.
            if lines[line - 1].trim_start().starts_with("//") {
                continue;
            }
            writes.push(Write {
                file: file.clone(),
                line,
                function: enclosing_fn(&lines, line),
                what: format!("{verb} {table}"),
            });
        }
    }
    writes
}

fn listed(file: &str, function: &str) -> bool {
    let in_table = LogSink::ALL.iter().any(|s| s.writers().contains(&(file, function)));
    in_table || ALLOWLIST.iter().any(|a| a.file == file && a.function == function)
}

/// The writes outside both the table and the allowlist, as failure lines.
fn uncovered(files: &[(String, String)]) -> Vec<String> {
    find_writes(files)
        .into_iter()
        .filter(|w| !listed(&w.file, &w.function))
        .map(|w| format!("{}:{} {} in fn {} is in no LogSink writer table and no allowlist entry", w.file, w.line, w.what, w.function))
        .collect()
}

fn source_tree() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut paths = Vec::new();
    rust_files(&root.join("src"), &mut paths);
    paths.sort();
    paths
        .into_iter()
        .map(|p| {
            let rel = p.strip_prefix(root).expect("under root").to_string_lossy().replace('\\', "/");
            (rel, std::fs::read_to_string(&p).expect("read"))
        })
        .collect()
}

#[test]
fn piir_sinks_cover_every_write() {
    let files = source_tree();
    let bad = uncovered(&files);
    assert!(bad.is_empty(), "writes into a sink that LogSink does not know:\n{}", bad.join("\n"));

    // The scan is not vacuous: it sees the writes the registry names.
    let writes = find_writes(&files);
    for sink in LogSink::ALL {
        for (file, function) in sink.writers() {
            assert!(
                writes.iter().any(|w| w.file == *file && w.function == *function),
                "{sink}: the scan finds no write in {file} fn {function}: the table entry is stale"
            );
            // A writer the table names passes the redaction.
            let text = &files.iter().find(|(f, _)| f == file).unwrap_or_else(|| panic!("{file} exists")).1;
            let body = fn_body(text, function).unwrap_or_else(|| panic!("{file} has fn {function}"));
            assert!(body.contains("write_through"), "{sink}: {file} fn {function} writes without LogSink::write_through");
        }
    }
    // An allowlist entry names a real write and carries its reason.
    for a in ALLOWLIST {
        assert!(!a.reason.trim().is_empty(), "{}:{} has no reason", a.file, a.function);
        assert!(
            writes.iter().any(|w| w.file == a.file && w.function == a.function),
            "allowlist entry {}:{} matches no write: it is stale",
            a.file,
            a.function
        );
        assert!(!LogSink::ALL.iter().any(|s| s.writers().contains(&(a.file, a.function))), "{} is in the table and the allowlist", a.function);
    }

    // The producer set a reviewer watches grow (103 at b9277c04).
    let producers = Regex::new(r#"stderr_tail|"traceback""#).expect("regex");
    let n: usize = files.iter().map(|(_, t)| t.lines().filter(|l| producers.is_match(l)).count()).sum();
    println!("piir_ac05: {} write sites in {} table entries and {} allowlist entries; {n} lines match stderr_tail|\"traceback\"", writes.len(), LogSink::ALL.iter().map(|s| s.writers().len()).sum::<usize>(), ALLOWLIST.len());
}

#[test]
fn piir_sinks_scan_fails_on_a_planted_unlisted_write() {
    let mut files = source_tree();
    assert!(uncovered(&files).is_empty(), "the real tree is covered");
    files.push((
        "src/planted_sink.rs".to_string(),
        "fn leak_it(conn: &Connection) {\n    conn.execute(\n        \"INSERT INTO logs (tenant_id, line) VALUES (?1, ?2)\",\n        params![1, \"x\"],\n    )\n}\n".to_string(),
    ));
    let bad = uncovered(&files);
    assert_eq!(bad.len(), 1, "{bad:?}");
    assert!(bad[0].starts_with("src/planted_sink.rs:3 "), "names the file and the line: {bad:?}");
    assert!(bad[0].contains("leak_it"), "{bad:?}");

    // A planted write in the registry's own file name is still checked by function.
    let mut files = source_tree();
    files.push((
        "src/db.rs".to_string(),
        "fn sneaky() {\n    conn.execute(\"INSERT INTO call_traces (call_id) VALUES (1)\", [])\n}\n".to_string(),
    ));
    let bad = uncovered(&files);
    assert!(bad.iter().any(|b| b.contains("fn sneaky")), "{bad:?}");
}
