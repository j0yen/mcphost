//! PRD-mcphost-owner-verification-paths
//! AC7 (P1) — Given an unverified tenant, When `host.quickstart` is read,
//! Then `next` names email, github and dns from the generated table.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::verify::VERIFICATION_METHODS;
use serde_json::json;

#[tokio::test]
async fn ownv_ac07_unverified_quickstart_next_lists_every_method_from_the_table() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Ownv AC7").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let quickstart = extract_structured(&client.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("quickstart"));
    let next = quickstart["next"].as_array().expect("next array");
    for (method, tool) in VERIFICATION_METHODS {
        assert!(
            next.iter().any(|step| step["method"] == *method && step["call"] == *tool),
            "next must name {method} via {tool}: {next:?}"
        );
    }
    let rendered = serde_json::to_string(next).unwrap();
    for word in ["email", "github", "dns"] {
        assert!(rendered.contains(word), "next must mention {word}");
    }

    // Once verified, the nudge goes away.
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.unwrap().unwrap();
    server.state.db.verify_owner(tenant.id, "dns".to_string(), "acme.dev".to_string()).await.unwrap();
    let after = extract_structured(&client.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("quickstart"));
    assert!(
        !after["next"].as_array().unwrap().iter().any(|step| step["method"].is_string()),
        "a verified tenant is not nudged: {}",
        after["next"]
    );
}
