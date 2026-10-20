//! PRD-mcphost-acting-for-in-ledger-and-audit
//! AC1 (P0) — Given a depth-2 child `deployer` under `lead-job-42`, When it
//! calls an own tool once, Then the `calls` row has `key_id` set and a
//! single query renders `acting_for == {key_id, label: "deployer",
//! parent_label: "lead-job-42", root_label: "lead-job-42", depth: 2}`, and
//! the response `_meta.acting_for` equals it.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn delegate(base: &str, client: &McpClient, label: &str, ttl_s: i64) -> McpClient {
    let out: Value = extract_structured(
        &client
            .tools_call(
                "host.key.delegate",
                json!({"scopes": ["tools:call"], "ttl_s": ttl_s, "label": label, "can_delegate": true}),
            )
            .await
            .unwrap_or_else(|e| panic!("delegate {label}: {e:?}")),
    );
    McpClient::with_bearer(base, out["key"].as_str().expect("key"))
}

#[tokio::test]
async fn actfor_ac01_child_call_row_renders_acting_for_and_meta_equals_it() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "ActFor Row Tenant").await;
    let primary = McpClient::with_bearer(&server.base_url, &key);
    primary
        .tools_call("host.tool.publish", json!({"name": "ping", "kind": "echo", "spec": {"schema": {"type": "object"}}}))
        .await
        .expect("publish");
    let lead = delegate(&server.base_url, &primary, "lead-job-42", 3000).await;
    let deployer = delegate(&server.base_url, &lead, "deployer", 2000).await;

    let res = deployer.tools_call(&format!("{ns}.ping"), json!({})).await.expect("own tool call");
    let meta = res["_meta"]["acting_for"].clone();

    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let (key_id, label, parent_label, root_label, depth): (i64, String, Option<String>, String, i64) = conn
        .query_row(
            "WITH RECURSIVE up(id, label, parent_key_id, depth, lvl) AS (\
               SELECT id, label, parent_key_id, depth, 0 FROM tenant_keys WHERE id = \
                 (SELECT key_id FROM calls WHERE key_id IS NOT NULL AND tool_name LIKE '%ping' ORDER BY id DESC LIMIT 1) \
               UNION ALL SELECT k.id, k.label, k.parent_key_id, k.depth, up.lvl + 1 \
               FROM tenant_keys k JOIN up ON k.id = up.parent_key_id) \
             SELECT (SELECT id FROM up WHERE lvl = 0), (SELECT label FROM up WHERE lvl = 0), \
               (SELECT label FROM up WHERE lvl = 1), (SELECT label FROM up ORDER BY lvl DESC LIMIT 1), \
               (SELECT depth FROM up WHERE lvl = 0)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .expect("the calls row carries key_id and the lineage renders in one query");
    let rendered = json!({
        "key_id": key_id, "label": label, "parent_label": parent_label, "root_label": root_label, "depth": depth,
    });
    assert_eq!(
        rendered,
        json!({"key_id": key_id, "label": "deployer", "parent_label": "lead-job-42", "root_label": "lead-job-42", "depth": 2}),
        "{rendered}"
    );
    assert_eq!(meta, rendered, "response _meta.acting_for must equal the rendered lineage: {res}");
}
