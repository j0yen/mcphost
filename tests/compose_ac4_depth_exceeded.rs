//! PRD-mcphost-composition AC4 (P0) — Given tools nested five deep, When
//! the top is called, Then the fifth level is refused with
//! `compose_depth_exceeded` naming 4, and the tree finalizes with that
//! error.
//!
//! `chain0 -> chain1 -> chain2 -> chain3 -> chain4 -> (chain5, published but
//! never reached)`: four successful nested dispatches land at
//! `compose_depth` 4; the fifth (into `chain4`'s own step, naming `chain5`)
//! is refused before `chain5` is even looked up (`kinds::compose_call`
//! checks the ceiling first) -- `chain5` is never dispatched.
//!
//! PRD-mcphost-chain-host-steps requirement 4 (landed after this PRD):
//! `host.tool_publish` now resolves every step's tool before publishing, so
//! every link needs its target to already exist -- `chain5` (a trivial
//! `echo` tool, purely to satisfy `chain4`'s own resolution) then
//! chain4..chain0 published in REVERSE order, each one naming an already-
//! published target. The depth ceiling still refuses before `chain5` is
//! ever dispatched, so its own kind doesn't matter.

use crate::common;
use common::{TestServer, chain_kind_registry, publish, signup};
use serde_json::json;

#[tokio::test]
async fn nested_five_deep_is_refused_at_the_fifth_level_naming_the_limit_of_four() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Depth Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    publish(
        &client,
        "chain5",
        "echo",
        json!({"schema": {"type": "object"}}),
    )
    .await;
    for (name, target) in [
        ("chain4", "chain5"),
        ("chain3", "chain4"),
        ("chain2", "chain3"),
        ("chain1", "chain2"),
        ("chain0", "chain1"),
    ] {
        publish(
            &client,
            name,
            "chain",
            json!({"steps": [{"tool": target, "args": {}}]}),
        )
        .await;
    }

    let err = client
        .tools_call(&format!("{ns}.chain0"), json!({}))
        .await
        .expect_err("a call nested five deep must be refused");
    assert_eq!(err.error_code.as_deref(), Some("compose_depth_exceeded"));
    assert_eq!(err.data["limit"], json!(4));
}
