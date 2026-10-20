//! PRD-mcphost-audit-export-tamper-evident
//! AC3 (P0) — Given the export, When `scripts/audit-verify.py` runs with the
//! test JWKS, Then `OK 9 rows`; Given one byte of line 5 altered, Then
//! `FAIL line 5` and exit 1; Given the trailer signature altered, Then
//! `FAIL signature`.

use crate::audx_ac02_export_nine_rows_three_tables::seed_nine;
use crate::common::{TestServer, extract_structured, signup};
use serde_json::json;
use std::path::Path;
use std::process::Command;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// A fresh scratch directory under the system temp dir (no `tempfile` dependency).
pub(crate) fn scratch_dir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("audx-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// Runs the verifier on `jsonl` against `jwks`; returns (exit code, stdout).
pub(crate) fn run_verifier(dir: &Path, jsonl: &str, jwks: &serde_json::Value) -> (i32, String) {
    let export = dir.join("export.jsonl");
    let jwks_path = dir.join("jwks.json");
    std::fs::write(&export, jsonl).expect("write export");
    std::fs::write(&jwks_path, jwks.to_string()).expect("write jwks");
    let out = Command::new("python3")
        .arg("-I")
        .arg(Path::new(ROOT).join("scripts/audit-verify.py"))
        .arg(&export)
        .arg("--jwks")
        .arg(&jwks_path)
        .output()
        .expect("run audit-verify.py");
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[tokio::test]
async fn audx_ac03_verifier_accepts_the_export_and_names_the_altered_line_and_signature() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Audx AC3").await;
    let (client, _) = seed_nine(&server, ns, &key).await;
    let jwks: serde_json::Value = reqwest::get(format!("{}/.well-known/jwks.json", server.base_url))
        .await
        .expect("jwks")
        .json()
        .await
        .expect("jwks json");
    let out = extract_structured(&client.tools_call("host.audit.export", json!({})).await.expect("export"));
    let jsonl = out["jsonl"].as_str().expect("jsonl").to_string();
    let dir = scratch_dir("ac03");

    let (code, stdout) = run_verifier(&dir, &jsonl, &jwks);
    assert_eq!((code, stdout.starts_with("OK 9 rows")), (0, true), "{stdout}");

    // One byte of physical line 5 altered (a subject letter).
    let mut lines: Vec<String> = jsonl.lines().map(str::to_string).collect();
    lines[4] = lines[4].replacen("\"subject\":\"", "\"subject\":\"x", 1);
    let tampered = format!("{}\n", lines.join("\n"));
    let (code, stdout) = run_verifier(&dir, &tampered, &jwks);
    assert_eq!((code, stdout.as_str()), (1, "FAIL line 5"));

    // The trailer signature altered.
    let mut lines: Vec<String> = jsonl.lines().map(str::to_string).collect();
    let last = lines.len() - 1;
    let sig = out["signature"].as_str().expect("signature");
    let flipped = format!("{}{}", if sig.starts_with('A') { "B" } else { "A" }, &sig[1..]);
    lines[last] = lines[last].replace(sig, &flipped);
    let (code, stdout) = run_verifier(&dir, &format!("{}\n", lines.join("\n")), &jwks);
    assert_eq!((code, stdout.as_str()), (1, "FAIL signature"));
}
