//! PRD-mcphost-example-from-published-tool
//! AC6 — Given a proposal for a name already in the embedded index, When
//! proposed, Then `example_exists`; given a 4th open proposal from one
//! tenant, Then `too_many_proposals`.

use crate::exprp;

use crate::common::{ADMIN_KEY, McpClient};
use serde_json::json;

#[tokio::test]
async fn a_name_already_in_the_embedded_index_is_example_exists() {
    let f = exprp::start().await;
    let existing = mcphost::examples::EXAMPLES.first().expect("the embedded index has examples").name;
    let tool = existing.replace('-', "_");
    exprp::publish_http(&f.client, &tool, exprp::probe_spec(&f.upstream)).await;

    let err = exprp::propose(&f.client, exprp::propose_args(&tool)).await.expect_err("the name is taken");
    assert_eq!(err.error_code.as_deref(), Some("example_exists"), "{} {}", err.code, err.message);
    assert_eq!(err.data["name"], existing);
    assert!(f.server.state.db.list_example_proposals(None).await.expect("list").is_empty());
}

#[tokio::test]
async fn a_fourth_open_proposal_is_too_many_until_one_is_decided() {
    let f = exprp::start().await;
    for name in ["probe_a", "probe_b", "probe_c", "probe_d"] {
        exprp::publish_http(&f.client, name, exprp::probe_spec(&f.upstream)).await;
    }
    let mut first = 0;
    for name in ["probe_a", "probe_b", "probe_c"] {
        let p = exprp::propose(&f.client, exprp::propose_args(name)).await.unwrap_or_else(|e| panic!("{name}: {} {}", e.code, e.message));
        first = first.max(p["proposal_id"].as_i64().unwrap());
    }
    let err = exprp::propose(&f.client, exprp::propose_args("probe_d")).await.expect_err("a 4th open proposal");
    assert_eq!(err.error_code.as_deref(), Some("too_many_proposals"), "{} {}", err.code, err.message);
    assert_eq!(err.data["limit"], 3);
    assert_eq!(f.server.state.db.list_example_proposals(None).await.expect("list").len(), 3);

    // A decision frees a slot.
    let admin = McpClient::with_bearer(&f.server.base_url, ADMIN_KEY);
    admin.tools_call("admin.example_proposals", json!({"action": "reject", "id": first, "reason": "dup"})).await.expect("reject");
    exprp::propose(&f.client, exprp::propose_args("probe_d")).await.expect("a slot is free again");
}
