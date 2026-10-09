//! PRD-mcphost-healthz-version-field AC4 — Given a binary built from a tree
//! with no `.git` directory, When `/healthz` is fetched, Then `git_sha` is
//! JSON `null` and `version` is still present.
//!
//! A full second build of the crate is not needed to prove the chain: (1)
//! `build.rs`, run in a `.git`-less directory, emits no `MCPHOST_GIT_SHA`
//! (so `option_env!` is `None`); (2) the real `/healthz` handler, served
//! from an `AppState` carrying a `BuildInfo` with `git_sha: None`,
//! renders JSON `null` beside `version`, never omitting the key.

use mcphost::build_info::BuildInfo;
use serde_json::json;
use std::process::Command;

#[test]
fn build_rs_in_gitless_tree_emits_no_sha() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let tmp = std::env::temp_dir().join(format!("hzver_ac04_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let exe = tmp.join("build_script");
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let status = Command::new(rustc)
        .args(["--edition", "2024", "-o"])
        .arg(&exe)
        .arg(format!("{manifest}/build.rs"))
        .status()
        .expect("compile build.rs");
    assert!(status.success());

    let workdir = tmp.join("tree");
    std::fs::create_dir_all(&workdir).unwrap();
    assert!(!workdir.join(".git").exists());
    let out = Command::new(&exe)
        .current_dir(&workdir)
        .env_remove("MCPHOST_GIT_SHA")
        .output()
        .expect("run build.rs");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("MCPHOST_GIT_SHA="),
        "no .git => no sha stamped: {stdout}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

/// The real `/healthz` handler, served over HTTP from an `AppState` whose
/// build identity is what a `.git`-less build produces (`git_sha: None`).
#[tokio::test]
async fn healthz_over_http_renders_null_git_sha_with_version() {
    static GITLESS: BuildInfo = BuildInfo { version: "9.9.9", git_sha: None };
    let dir = crate::common::TempDataDir::new();
    let mut state = crate::common::bare_state(&dir.0).await;
    state.build = &GITLESS;
    let app = mcphost::http::build_router(std::sync::Arc::new(state));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let resp = reqwest::get(format!("http://{addr}/healthz")).await.expect("GET /healthz");
    let body: serde_json::Value = resp.json().await.expect("parse /healthz");
    assert_eq!(body["git_sha"], json!(null), "body: {body}");
    assert_eq!(body["version"], json!("9.9.9"), "body: {body}");
    assert!(body.as_object().unwrap().contains_key("git_sha"), "key present: {body}");
}
