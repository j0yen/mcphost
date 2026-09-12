//! PRD-mcphost-synthetic-flag
//! AC9 (P1) — Given the metering listing with `synthetic: false`, When
//! read, Then only unlabeled tenants' rows return; `true` returns only
//! labeled; `all` returns both.
//!
//! `admin.tenants` is this host's listing equivalent (requirement 5's
//! parenthetical) -- there is no separate `admin.metering` tool.
//!
//! PRD-mcphost-tenant-attribution requirement 1: every loopback signup now
//! defaults `synthetic` to `harness:unstamped` (never null) -- this
//! suite's test server is always loopback-sourced, so a genuinely
//! unlabeled (`null`) tenant here needs a genuinely `external` signup,
//! made via `control::signup` directly (bypassing the HTTP layer, same as
//! `attrib_ac4`) rather than the `signup()` HTTP helper.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn synthetic_filter_splits_true_false_all() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let (ns_labeled, _) = signup(&server.base_url, "Filter Labeled").await;
    let unlabeled_result = mcphost::control::signup(
        &server.state,
        &json!({"name": "Filter Unlabeled"}),
        "8.8.8.8",
        mcphost::control::SignupAttribution::default(),
    )
    .await
    .expect("external signup");
    let ns_unlabeled = unlabeled_result["tenant"]
        .as_str()
        .expect("tenant field")
        .to_string();
    admin
        .tools_call(
            "admin.tenant_set_synthetic",
            json!({"tenant": ns_labeled, "label": "synthorg:ac9"}),
        )
        .await
        .expect("label tenant");

    async fn namespaces(admin: &McpClient, synthetic: &str) -> Vec<String> {
        let result = admin
            .tools_call("admin.tenants", json!({"synthetic": synthetic}))
            .await
            .expect("admin.tenants");
        extract_structured(&result)["tenants"]
            .as_array()
            .expect("tenants array")
            .iter()
            .map(|t| t["tenant"].as_str().unwrap().to_string())
            .collect()
    }

    let only_true = namespaces(&admin, "true").await;
    assert!(only_true.contains(&ns_labeled));
    assert!(!only_true.contains(&ns_unlabeled));

    let only_false = namespaces(&admin, "false").await;
    assert!(!only_false.contains(&ns_labeled));
    assert!(only_false.contains(&ns_unlabeled));

    let all = namespaces(&admin, "all").await;
    assert!(all.contains(&ns_labeled));
    assert!(all.contains(&ns_unlabeled));

    // Default (no `synthetic` argument at all) must behave like "all".
    let default_result = admin
        .tools_call("admin.tenants", json!({}))
        .await
        .expect("admin.tenants default");
    let default_ns: Vec<String> = extract_structured(&default_result)["tenants"]
        .as_array()
        .expect("tenants array")
        .iter()
        .map(|t| t["tenant"].as_str().unwrap().to_string())
        .collect();
    assert!(default_ns.contains(&ns_labeled));
    assert!(default_ns.contains(&ns_unlabeled));
}
