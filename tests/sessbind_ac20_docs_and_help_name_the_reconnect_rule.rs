//! PRD-mcphost-session-bound-tenant-key
//! AC7 (P1) — Given the docs build, When `docs/` is regenerated, Then the
//! reconnect rule appears under the one-URL onboarding section and
//! `help.rs`'s cause text names it.

use crate::common::TestServer;

const AGENT_QUICKSTART_MD: &str = include_str!("../docs/agent-quickstart.md");

#[test]
fn quickstart_one_url_section_names_the_reconnect_rule() {
    let heading = "## Quickstart for agents";
    let start = AGENT_QUICKSTART_MD
        .find(heading)
        .unwrap_or_else(|| panic!("docs/agent-quickstart.md must have a '{heading}' heading"));
    let next_heading = "\n## ";
    let end = AGENT_QUICKSTART_MD[start + heading.len()..]
        .find(next_heading)
        .map(|i| start + heading.len() + i)
        .unwrap_or(AGENT_QUICKSTART_MD.len());
    let section = &AGENT_QUICKSTART_MD[start..end];

    assert!(
        section.contains("Reconnect rule"),
        "the one-URL onboarding section must name the reconnect rule: {section}"
    );
    assert!(
        section.contains("refused") && section.contains("data.tenant"),
        "the reconnect rule must say a later key-less call is refused and named: {section}"
    );
    assert!(
        section.contains("never creates a second tenant") || section.contains("never create a second tenant"),
        "the reconnect rule must say it never creates a second tenant: {section}"
    );
}

#[test]
fn help_rs_tenant_key_missing_cause_names_the_reconnect_rule() {
    let entry = mcphost::help::HELP_ENTRIES
        .iter()
        .find(|e| e.code == "tenant_key_missing")
        .expect("tenant_key_missing has a HelpEntry");
    assert!(
        entry.cause.contains("already named a tenant") || entry.cause.contains("already is that tenant"),
        "tenant_key_missing's cause text must name the reconnect rule: {}",
        entry.cause
    );
    assert!(
        entry.fix.contains("reconnect") || entry.fix.contains("already is that tenant"),
        "tenant_key_missing's fix text must point at reconnecting: {}",
        entry.fix
    );
}

#[tokio::test]
async fn help_page_serves_the_reconnect_rule_too() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();

    let resp = http
        .get(format!("{}/help/tenant_key_missing", server.base_url))
        .send()
        .await
        .expect("GET /help/tenant_key_missing");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp.text().await.expect("read body");

    assert!(
        body.contains("already named a tenant") || body.contains("already is that tenant"),
        "/help/tenant_key_missing must serve the reconnect rule: {body}"
    );
    // The pre-existing claim (PRD-mcphost-docs-one-url-flow AC7) must still
    // hold too -- this AC narrows it, never removes it.
    assert!(
        body.contains("no key is needed on the bare /mcp endpoint") || body.contains("no key is needed on /mcp"),
        "/help/tenant_key_missing must still say no key is needed on /mcp: {body}"
    );
}
