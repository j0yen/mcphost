//! PRD-mcphost-unknown-import-domain-hint
//! AC6 (P0) -- Given `mcphost llms-txt`, When run, Then `www/llms.txt`
//! contains the generated line before the first `import mcphost` example,
//! and `llms-txt --check` fails if the line is removed.

use crate::xlinks_cli;

const LINE_START: &str = "mcphost.dev is the server's address; the Python module is 'import mcphost' (";

/// Remove the generated block (markers and line) from `doc`.
fn without_block(doc: &str) -> String {
    let start = doc.find("<!-- domain-hint:start -->").expect("start marker");
    let end_marker = "<!-- domain-hint:end -->\n";
    let end = doc.find(end_marker).expect("end marker") + end_marker.len();
    format!("{}{}", &doc[..start], &doc[end..])
}

#[test]
fn llms_txt_run_renders_the_line_before_the_first_example_and_check_catches_removal() {
    let s = xlinks_cli::Scratch::new("domhint-ac06");
    let committed = s.read("llms.txt");
    // Start from a file without the block: running the generator must put it back.
    s.write("llms.txt", &without_block(&committed));
    let out = s.llms_txt(false);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let doc = s.read("llms.txt");

    let line_at = doc.find(LINE_START).expect("generated line is rendered");
    let after_line = line_at + LINE_START.len();
    let example_at = after_line + doc[after_line..].find("import mcphost").expect("an example follows");
    assert!(line_at < example_at);
    assert!(doc[..line_at].find("import mcphost").is_none(), "no example may precede the line");
    // The committed file is already regenerated.
    assert_eq!(doc, committed, "the committed www/llms.txt is stale: run `mcphost llms-txt`");

    // Removing the line makes --check fail, naming the file, without writing.
    let removed = without_block(&doc);
    s.write("llms.txt", &removed);
    let check = s.llms_txt(true);
    assert_eq!(check.status.code(), Some(1), "{}", String::from_utf8_lossy(&check.stderr));
    assert!(String::from_utf8_lossy(&check.stderr).contains(&s.path("llms.txt").display().to_string()));
    assert_eq!(s.read("llms.txt"), removed, "--check must not write");
}
