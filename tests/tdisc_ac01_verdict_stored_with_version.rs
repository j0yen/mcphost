//! PRD-mcphost-trusted-discovery-catalog-receipts AC1 (P0) -- a host-run test
//! (`host.tool_test`, or `require_tested` on `host.tool_share`) stores its
//! verdict with the version it judged; `tested_version` moves exactly as
//! before; after a republish the entry shows `test_verdict` null until the
//! tool is retested.

use crate::common;
use common::{McpClient, TestServer, chain_kind_registry, extract_structured, publish, publish_forced, signup};
use rusqlite::Connection;
use serde_json::{Value, json};

/// `(test_verdict, test_verdict_version, tested_version, current_version)` of a tool row.
fn stored(server: &TestServer, name: &str) -> (Option<String>, Option<i64>, Option<i64>, i64) {
    Connection::open(server.data_dir.0.join("mcphost.db"))
        .unwrap()
        .query_row(
            "SELECT test_verdict, test_verdict_version, tested_version, current_version FROM tools WHERE name = ?1",
            [name],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap()
}

fn chain(mapping: &str) -> Value {
    json!({"steps": [
        {"tool": "step1", "args": {"leads": [{"name": "a"}]}},
        {"tool": "step2", "args": {"lead": mapping}},
    ]})
}

async fn entry(client: &McpClient, full_name: &str) -> Value {
    let found = extract_structured(&client.tools_call("host.catalog.search", json!({"q": "tdisc ac1"})).await.expect("search"));
    let tools = found["tools"].as_array().expect("tools");
    tools.iter().find(|t| t["name"] == json!(full_name)).unwrap_or_else(|| panic!("{full_name} missing: {found}")).clone()
}

#[tokio::test]
async fn tdisc_ac01() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Tdisc AC1 Owner").await;
    let owner = McpClient::with_bearer(&server.base_url, &key);
    let (_ns_b, key_b) = signup(&server.base_url, "Tdisc AC1 Reader").await;
    let reader = McpClient::with_bearer(&server.base_url, &key_b);
    let schema = json!({"type": "object"});
    publish(&owner, "step1", "echo", json!({"schema": schema})).await;
    publish(&owner, "step2", "echo", json!({"schema": schema})).await;

    // pass via host.tool_test: stored with the version it judged.
    let full = publish(&owner, "pipe", "chain", chain("$.prev.result.leads")).await;
    assert_eq!(stored(&server, "pipe"), (None, None, None, 1), "nothing stored before a test");
    let tested = extract_structured(&owner.tools_call("host.tool_test", json!({"name": "pipe", "args": {}})).await.expect("test"));
    assert_eq!(tested["verdict"], json!("pass"), "{tested}");
    assert_eq!(tested["tested_version"], json!(1), "{tested}");
    assert_eq!(stored(&server, "pipe"), (Some("pass".into()), Some(1), Some(1), 1));

    owner
        .tools_call("host.tool_share", json!({"name": "pipe", "visibility": "public", "description": "tdisc ac1 pipe"}))
        .await
        .expect("share");
    let e = entry(&reader, &full).await;
    assert_eq!(e["test_verdict"], json!("pass"), "{e}");
    assert_eq!(e["stale_test"], json!(false), "{e}");

    // republish: both columns stay, the entry hides the verdict, the tool is stale.
    publish_forced(&owner, "pipe", "chain", chain("$.prev.result.leadz")).await;
    assert_eq!(stored(&server, "pipe"), (Some("pass".into()), Some(1), Some(1), 2));
    let e = entry(&reader, &full).await;
    assert_eq!(e["test_verdict"], Value::Null, "a verdict of version 1 is not shown for version 2: {e}");
    assert_eq!(e["stale_test"], json!(true), "{e}");

    // will_fail via host.tool_test on the current version: stored, never stamps.
    let tested = extract_structured(&owner.tools_call("host.tool_test", json!({"name": "pipe", "args": {}})).await.expect("test"));
    assert_eq!(tested["verdict"], json!("will_fail"), "{tested}");
    assert_eq!(stored(&server, "pipe"), (Some("will_fail".into()), Some(2), Some(1), 2), "tested_version stays 1");
    let e = entry(&reader, &full).await;
    assert_eq!(e["test_verdict"], json!("will_fail"), "{e}");
    assert_eq!(e["tested_version"], json!(1), "{e}");
    assert_eq!(e["stale_test"], json!(true), "{e}");

    // require_tested on host.tool_share: a will_fail version refuses and stores the verdict.
    publish_forced(&owner, "gate", "chain", chain("$.prev.result.leadz")).await;
    let refused = owner
        .tools_call("host.tool_share", json!({"name": "gate", "visibility": "public", "description": "tdisc ac1 gate"}))
        .await;
    assert!(refused.is_err(), "a will_fail tool cannot be shared public");
    assert_eq!(stored(&server, "gate"), (Some("will_fail".into()), Some(1), None, 1));

    // require_tested on a passing version stores `pass` with tested_version.
    publish(&owner, "gate2", "chain", chain("$.prev.result.leads")).await;
    owner
        .tools_call("host.tool_share", json!({"name": "gate2", "visibility": "public", "description": "tdisc ac1 gate2"}))
        .await
        .expect("share gate2");
    assert_eq!(stored(&server, "gate2"), (Some("pass".into()), Some(1), Some(1), 1));

    // retest after the republish: the pass of version 2 shows again.
    publish_forced(&owner, "pipe", "chain", chain("$.prev.result.leads")).await;
    let e = entry(&reader, &full).await;
    assert_eq!(e["test_verdict"], Value::Null, "version 3 is untested: {e}");
    owner.tools_call("host.tool_test", json!({"name": "pipe", "args": {}})).await.expect("retest");
    assert_eq!(stored(&server, "pipe"), (Some("pass".into()), Some(3), Some(3), 3));
    let e = entry(&reader, &full).await;
    assert_eq!(e["test_verdict"], json!("pass"), "{e}");

    // the writer takes the verdict as data: `unverifiable` round-trips.
    let tenant_id: i64 = Connection::open(server.data_dir.0.join("mcphost.db"))
        .unwrap()
        .query_row("SELECT tenant_id FROM tools WHERE name = 'pipe'", [], |r| r.get(0))
        .unwrap();
    server.state.db.record_test_verdict(tenant_id, "pipe".into(), 3, "unverifiable".into()).await.unwrap();
    assert_eq!(stored(&server, "pipe"), (Some("unverifiable".into()), Some(3), Some(3), 3));
    server.state.db.record_test_verdict(tenant_id, "pipe".into(), 2, "pass".into()).await.unwrap();
    assert_eq!(stored(&server, "pipe").0.as_deref(), Some("unverifiable"), "a verdict for a past version is not written");
}
