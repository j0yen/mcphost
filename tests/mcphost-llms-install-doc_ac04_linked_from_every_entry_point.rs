//! AC4 (PRD-mcphost-llms-install-doc) — Given `www/llms.txt`,
//! `www/skill.md`, and the README, When read, Then each contains the
//! absolute URL `https://mcphost.dev/llms-install.md`; `host.quickstart`
//! returns `install_doc_url` equal to it.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use mcphost::install_links;
use serde_json::json;

const URL: &str = "https://mcphost.dev/llms-install.md";

#[test]
fn llms_txt_skill_md_and_readme_carry_the_absolute_url() {
    for (name, text) in [
        ("www/llms.txt", include_str!("../www/llms.txt")),
        ("www/skill.md", include_str!("../www/skill.md")),
        ("README.md", include_str!("../README.md")),
        ("plugin/skills/mcphost/SKILL.md", include_str!("../plugin/skills/mcphost/SKILL.md")),
    ] {
        assert!(text.contains(URL), "{name} must contain {URL}");
    }
    assert_eq!(install_links::install_doc_url(), URL);
}

#[tokio::test]
async fn quickstart_returns_install_doc_url_signed_in_and_out() {
    let server = TestServer::start().await;

    let anon = extract_structured(
        &McpClient::new(&server.base_url)
            .tools_call("host.quickstart", json!({"kind": "echo"}))
            .await
            .expect("anonymous host.quickstart"),
    );
    assert_eq!(anon["install_doc_url"], URL, "{anon}");

    let session = McpClient::new(&server.base_url).with_session_continuity();
    session.tools_call("signup", json!({"name": "AC4 Tenant"})).await.expect("signup");
    let signed_in = extract_structured(
        &session.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("host.quickstart"),
    );
    assert_eq!(signed_in["install_doc_url"], URL, "{signed_in}");
}
