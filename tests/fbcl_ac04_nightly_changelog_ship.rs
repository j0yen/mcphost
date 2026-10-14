//! PRD-mcphost-feedback-closed-loop-notice AC4 — Given a `CHANGELOG.md`
//! fixture with `- fixed args error text (theme:th_0123456789ab) anchor:args`
//! under `## 0.110.0`, When the nightly task runs, Then the theme is
//! `shipped` with `shipped_version: "0.110.0"` and `shipped_anchor: "args"`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, fbcl_themed};
use serde_json::json;

#[tokio::test]
async fn nightly_pass_ships_the_theme_a_changelog_line_names() {
    let server = TestServer::start().await;
    let (_clients, _ids, theme_id) = fbcl_themed(&server, &["fbcl4-a"], 0).await;
    let changelog = format!(
        "# Changelog\n\n- fixed before any heading (theme:{theme_id}) anchor:nope\n\n## 0.110.0\n\n\
         - unrelated line\n- fixed args error text (theme:{theme_id}) anchor:args\n\
         - ghost theme (theme:th_0123456789ab) anchor:args\n"
    );
    let shipped = mcphost::feedback::run_nightly_pass(&server.state, &changelog).await.unwrap();
    assert_eq!(shipped, vec![theme_id.clone()], "the ghost theme does not exist and is skipped");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let out = extract_structured(&admin.tools_call("admin.feedback.themes", json!({"status": "shipped"})).await.unwrap());
    let themes = out["themes"].as_array().unwrap();
    assert_eq!(themes.len(), 1, "{out}");
    assert_eq!(themes[0]["status"], "shipped");
    assert_eq!(themes[0]["shipped_version"], "0.110.0");
    assert_eq!(themes[0]["shipped_anchor"], "args");

    // A second night finds it already shipped.
    assert!(mcphost::feedback::run_nightly_pass(&server.state, &changelog).await.unwrap().is_empty());
}

#[test]
fn scan_reads_v_prefixed_dated_headings_and_optional_anchor() {
    let got = mcphost::feedback::scan_changelog("## v0.107.72 — 2026-10-13\n- x (theme:th_0123456789ab)\n");
    assert_eq!(got.len(), 1);
    assert_eq!((got[0].version.as_str(), got[0].anchor.as_deref()), ("0.107.72", None));
}
