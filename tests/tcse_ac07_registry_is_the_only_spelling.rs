//! PRD-mcphost-table-conflict-structured-error
//! contract: walks src/
//! AC7 -- Given the error registry, When the SDK generators and
//! `sandbox-api-doc-check.sh` run, Then the python and TS SDKs expose
//! `TableRowConflict` and the doc lists `table_row_conflict`; the registry row
//! is the only place the code is spelled (a grep of `src/` for the literal
//! outside `err_code!` finds nothing).

const CODE: &str = "table_row_conflict";

fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read src dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|x| x == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn tcse_ac07() {
    // The registry row exists and both SDK generators derive their class from it.
    let row = mcphost::error_registry::lookup(CODE).expect("registry row for table_row_conflict");
    assert_eq!(row.code, CODE);
    assert!(mcphost::sdk_python::error_codes().iter().any(|c| c == CODE));
    let contract = mcphost::call_cli::EMBEDDED_CONTRACT;
    let py = mcphost::sdk_python::render(contract).expect("render python sdk");
    let py_errors = py.iter().find(|(k, _)| k.ends_with("errors.py")).expect("errors.py").1;
    assert!(py_errors.contains("class TableRowConflict(MCPHostError)"), "python SDK lacks TableRowConflict");
    let ts = mcphost::r#gen::sdk_ts::render(contract).expect("render ts sdk");
    let ts_errors = ts.iter().find(|(k, _)| k.ends_with("errors.ts")).expect("errors.ts").1;
    assert!(ts_errors.contains("export class TableRowConflict extends MCPHostError"), "TS SDK lacks TableRowConflict");

    // The generated doc lists it, and the doc check agrees the doc is in sync.
    let doc = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/sandbox-api.md")).expect("read doc");
    assert!(doc.contains(&format!("`{CODE}`")), "docs/sandbox-api.md must list {CODE}");
    let out = std::process::Command::new("bash")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/sandbox-api-doc-check.sh"))
        .env("MCPHOST_BIN", env!("CARGO_BIN_EXE_mcphost"))
        .output()
        .expect("run sandbox-api-doc-check.sh");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    // One spelling: the `err_code!` declaration, nowhere else under src/.
    let mut files = Vec::new();
    rust_files(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
    let needle = format!("\"{CODE}\"");
    let mut hits = Vec::new();
    for f in &files {
        let text = std::fs::read_to_string(f).expect("read source");
        for (i, line) in text.lines().enumerate() {
            if line.contains(&needle) && !line.trim_start().starts_with("//") {
                hits.push(format!("{}:{}: {}", f.display(), i + 1, line.trim()));
            }
        }
    }
    assert_eq!(hits.len(), 1, "exactly one spelling expected: {hits:#?}");
    assert!(hits[0].contains("src/tables.rs") && hits[0].contains("code: "), "{hits:#?}");
}
