//! PRD-mcphost-push-from-directory
//! AC7 (P1) — Given `--watch`, When `tool.py` changes, Then one push runs
//! within 2 s and an unchanged save produces no output.

use crate::common;
use common::{TestServer, ToolDirFixture, python_kind_registry, tenant_url};
use mcphost::sandbox;
use std::time::Duration;
use tokio::process::Command;

use crate::serve;
use serve::ServeLines;

#[tokio::test]
async fn pshd_ac07_edit_pushes_once_within_2s_and_an_unchanged_save_is_silent() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let data = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&data.0)).await;
    let url = tenant_url(&server.base_url).await;
    let dir = ToolDirFixture::from_example("hello-py");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mcphost"));
    cmd.arg("push")
        .arg(&dir.0)
        .args(["--watch", "--url", &url])
        .env_remove("MCPHOST_URL");
    let mut lines = ServeLines::spawn_lines(cmd);

    let first = lines
        .next_line(Duration::from_secs(30))
        .await
        .expect("the initial push prints")
        .expect("a line");
    assert!(first.contains("published hello_py v1"), "{first}");

    // An edit: one push within 2 s.
    let source = std::fs::read_to_string(dir.0.join("tool.py")).unwrap();
    dir.write("tool.py", &format!("{source}# edited\n"));
    let second = lines
        .next_line(Duration::from_secs(2))
        .await
        .expect("a push runs within 2 s of the edit")
        .expect("a line");
    assert!(second.contains("published hello_py v2"), "{second}");

    // Saving the same bytes again prints nothing.
    dir.write("tool.py", &format!("{source}# edited\n"));
    let quiet = lines.next_line(Duration::from_millis(1500)).await;
    assert!(quiet.is_err(), "an unchanged save is silent, got {quiet:?}");
}
