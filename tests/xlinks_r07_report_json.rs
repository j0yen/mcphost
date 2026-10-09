//! R7 (PRD-mcphost-docs-external-links-resolve, P1; no AC of its own) --
//! `docs-link-check.sh --report` writes `{url, status, files}` rows for the
//! promise link to consume.

use crate::xlinks;

#[test]
fn report_lists_url_status_and_files() {
    let port = xlinks::serve();
    let dir = xlinks::scratch("r07");
    let file = dir.join("doc.md");
    let (ok, gone) = (format!("http://127.0.0.1:{port}/ok"), format!("http://127.0.0.1:{port}/gone"));
    std::fs::write(&file, format!("{ok}\n{gone}\nhttps://api.example.com/x\n")).unwrap();
    let report = dir.join("report.json");

    let out = std::process::Command::new(xlinks::repo_root().join("scripts/docs-link-check.sh"))
        .arg("--report")
        .arg(&file)
        .env("DOCS_LINK_REPORT", &report)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let rows: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
    let row = |u: &str| rows.as_array().unwrap().iter().find(|r| r["url"] == u).cloned().unwrap();
    assert_eq!(row(&ok)["status"], 200);
    assert_eq!(row(&gone)["status"], 404);
    assert_eq!(row("https://api.example.com/x")["status"], "skipped");
    assert_eq!(row(&ok)["files"], serde_json::json!([file.display().to_string()]));
}
