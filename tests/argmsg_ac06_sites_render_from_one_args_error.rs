//! PRD-mcphost-args-invalid-message-from-data
//! AC6 — Given the source tree, When the test scans `src/` for
//! `"args_invalid"` sites, Then each builds message and data from one
//! `ArgsError` and none calls `e.to_string()` for the message.

use std::path::{Path, PathBuf};

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rs_files(&path, out);
        } else if path.extension().is_some_and(|x| x == "rs") {
            out.push(path);
        }
    }
}

/// A site that emits the schema-validation `args_invalid`: a `Structured`
/// error literal (`code: "args_invalid"`) or a `structured_with` call whose
/// code argument is `"args_invalid"` (same line or the line after the open
/// paren). Other `"args_invalid"` mentions (docs, `InvalidArgs` mapping,
/// plain-message rejections) carry no jsonschema error and are not sites.
fn is_site(lines: &[&str], i: usize) -> bool {
    let l = lines[i];
    if l.contains("code: \"args_invalid\"") {
        return true;
    }
    let trimmed = l.trim();
    if l.contains("structured_with(\"args_invalid\"") {
        return true;
    }
    trimmed == "\"args_invalid\"," && i > 0 && lines[i - 1].trim_end().ends_with("structured_with(")
}

#[test]
fn every_args_invalid_site_renders_message_and_data_from_one_args_error() {
    let mut files = Vec::new();
    rs_files(Path::new(env!("CARGO_MANIFEST_DIR")).join("src").as_path(), &mut files);
    let mut sites = 0;
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        for i in 0..lines.len() {
            if !is_site(&lines, i) {
                continue;
            }
            sites += 1;
            let lo = i.saturating_sub(8);
            let hi = (i + 6).min(lines.len() - 1);
            let window = lines[lo..=hi].join("\n");
            let at = format!("{}:{}", file.display(), i + 1);
            assert!(!window.contains("e.to_string()"), "{at}: message from e.to_string()");
            assert!(window.contains("args_err.message()"), "{at}: no ArgsError::message()");
            assert!(window.contains("args_err.data()"), "{at}: no ArgsError data");
        }
    }
    assert!(sites >= 8, "expected the known args_invalid sites, found {sites}");
}
