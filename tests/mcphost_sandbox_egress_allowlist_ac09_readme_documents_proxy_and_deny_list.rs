//! PRD-mcphost-sandbox-egress-allowlist
//! AC9 (P1) — Given README on the built tree, When grepped, Then it
//! documents `MCPHOST_EGRESS_PROXY` and lists 169.254.0.0/16 in the deny
//! list.

const README: &str = include_str!("../README.md");

#[test]
fn readme_documents_the_egress_proxy_and_its_deny_list() {
    assert!(
        README.contains("MCPHOST_EGRESS_PROXY"),
        "README.md must document MCPHOST_EGRESS_PROXY"
    );
    assert!(
        README.contains("169.254.0.0/16"),
        "README.md must list 169.254.0.0/16 in the deploy-side deny list"
    );
}
