//! PRD-mcphost-openapi-for-rest-facade-b
//! AC2 (P1) -- Given `mcphost gen openapi --out /tmp/o.json`, When diffed
//! against `docs/openapi.json`, Then they are equal; one summary edited in
//! `ROUTES` without regenerating fails the check naming the file.
//!
//! `info.version` is the crate version and `wm-build land` bumps it after the
//! file was committed, so equality ignores that key and `x-contract-version` (a rebase onto a
//! changed contract moves it).

use mcphost::http::routes::ROUTES;
use serde_json::Value;
use std::process::Command;

fn mcphost() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_mcphost"));
    c.current_dir(env!("CARGO_MANIFEST_DIR"));
    c
}

fn load_sans_version(path: &std::path::Path) -> Value {
    let mut doc: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("read")).expect("json");
    doc["info"].as_object_mut().expect("info").remove("version");
    doc["info"].as_object_mut().expect("info").remove("x-contract-version");
    doc
}

#[test]
fn oapib_ac02() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("oapib_ac02");
    std::fs::create_dir_all(&dir).expect("tmp dir");
    let generated = dir.join("o.json");
    let out = mcphost().args(["gen", "openapi", "--out"]).arg(&generated).output().expect("gen openapi");
    assert!(out.status.success(), "gen openapi failed: {}", String::from_utf8_lossy(&out.stderr));

    let committed = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/openapi.json");
    assert_eq!(load_sans_version(&generated), load_sans_version(&committed), "docs/openapi.json differs from the live document");

    // The committed file passes the check.
    let ok = mcphost().args(["gen", "openapi", "--check", "--out"]).arg(&committed).output().expect("check");
    assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stderr));

    // One ROUTES summary edited without regenerating: the committed copy now
    // disagrees, and the check fails naming the file.
    let route = ROUTES.iter().find(|r| r.path == "/healthz").expect("/healthz in ROUTES");
    let mut stale: Value = serde_json::from_str(&std::fs::read_to_string(&generated).unwrap()).unwrap();
    let op = &mut stale["paths"]["/healthz"][route.method.to_lowercase()];
    assert_eq!(op["summary"], route.summary);
    op["summary"] = Value::String(format!("{} (edited)", route.summary));
    let stale_path = dir.join("stale-openapi.json");
    std::fs::write(&stale_path, serde_json::to_vec_pretty(&stale).unwrap()).unwrap();
    let bad = mcphost().args(["gen", "openapi", "--check", "--out"]).arg(&stale_path).output().expect("check stale");
    assert_eq!(bad.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&bad.stderr);
    assert!(stderr.contains("stale-openapi.json"), "stderr must name the file: {stderr}");
}
