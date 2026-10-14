//! PRD-mcphost-dry-run-on-write-verbs
//! AC6 -- Given `host.key.rotate {dry_run: true}`, When called, Then it fails
//! with `args_invalid` and `data.no_dry_run_reason == "external_effect"`, and
//! `docs/host-tools.md` contains that reason beside `host.key.rotate`
//! (generated, checked by `scripts/sandbox-api-doc-check.sh`).

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;
use std::path::Path;
use std::process::Command;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[tokio::test]
async fn key_rotate_dry_run_is_refused_with_its_reason_and_the_key_still_works() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Dryv AC6 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call("host.key.rotate", json!({"dry_run": true}))
        .await
        .expect_err("host.key.rotate refuses dry_run");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    assert_eq!(err.data["no_dry_run_reason"], json!("external_effect"), "{err:?}");

    // The refusal rotated nothing: the same key still authenticates.
    client.tools_call("host.whoami", json!({})).await.expect("key still valid");
}

#[test]
fn host_tools_doc_names_the_reason_beside_the_verb_and_the_check_script_catches_drift() {
    let doc = std::fs::read_to_string(repo_root().join("docs/host-tools.md")).expect("docs/host-tools.md");
    let line = doc
        .lines()
        .find(|l| l.contains("`host.key.rotate`"))
        .expect("host.key.rotate has a row");
    assert!(line.contains("external_effect"), "{line}");

    // Run the real check script against a scratch copy of what it reads.
    let dir = std::env::temp_dir().join(format!("mcphost-dryv-ac06-{}-{}", std::process::id(), mcphost::state::now_unix_ms()));
    for rel in [
        "scripts/sandbox-api-doc-check.sh",
        "scripts/host-tools-doc-check.sh",
        "src/write_verbs.rs",
        "src/kinds/python.rs",
        "docs/host-tools.md",
        "docs/kinds/python.md",
        "www/llms.txt",
        "plugin/skills/mcphost/SKILL.md",
        "src/oauth.rs",
        // PRD-mcphost-oauth-first-entry-url: the script also checks that every
        // client page names the one OAUTH_ENTRY_PATH.
        "README.md",
        "llms-install.md",
        "www/llms-install.md",
        "www/llms-full.txt",
        "www/index.html",
        "docs/oauth-connect.md",
    ] {
        let dest = dir.join(rel);
        std::fs::create_dir_all(dest.parent().expect("parent")).expect("mkdir");
        std::fs::copy(repo_root().join(rel), &dest).unwrap_or_else(|e| panic!("copy {rel}: {e}"));
    }
    let run = |args: &[&str]| {
        Command::new("bash")
            .arg(dir.join("scripts/sandbox-api-doc-check.sh"))
            .args(args)
            .output()
            .expect("run script")
    };
    let ok = run(&[]);
    assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stderr));

    let doc_path = dir.join("docs/host-tools.md");
    let tampered = std::fs::read_to_string(&doc_path).expect("read").replace("`external_effect`", "`nothing`");
    std::fs::write(&doc_path, tampered).expect("tamper");
    let bad = run(&[]);
    assert_eq!(bad.status.code(), Some(1), "{}", String::from_utf8_lossy(&bad.stderr));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("host-tools.md"), "{}", String::from_utf8_lossy(&bad.stderr));

    let fixed = run(&["--write"]);
    assert!(fixed.status.success(), "{}", String::from_utf8_lossy(&fixed.stderr));
    assert!(run(&[]).status.success(), "--write regenerates the doc");
    let _ = std::fs::remove_dir_all(&dir);
}
