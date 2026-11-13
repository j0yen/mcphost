//! PRD-mcphost-org-shared-tools-namespace-v2
//! AC1 — Given an org with owner O and member M and O's tool `t` shared with
//! `visibility: org`, When M calls `org.t`, Then it runs under O's tenant, the
//! `calls` row records `caller_tenant_id` = M, no second `tools` row exists,
//! and no per-member copy was written.

use crate::common;
use common::{McpClient, TestServer, extract_structured, publish, signup};
use serde_json::json;

const ECHO: fn() -> serde_json::Value = || json!({"schema": {"type": "object"}});

#[tokio::test]
async fn orgs_ac01() {
    let server = TestServer::start().await;
    let (owner_ns, owner_key) = signup(&server.base_url, "Org Owner").await;
    let owner = McpClient::with_bearer(&server.base_url, &owner_key);
    let created = extract_structured(&owner.tools_call("host.org.create", json!({"name": "acme"})).await.expect("create"));
    let code = created["invite_url"]
        .as_str()
        .expect("invite_url")
        .trim_end_matches("/mcp")
        .rsplit('/')
        .next()
        .expect("code")
        .to_string();
    let (member_ns, member_key) = signup(&server.base_url, "Member").await;
    let member = McpClient::with_bearer(&server.base_url, &member_key);
    member.tools_call("host.org.join", json!({"code": code})).await.expect("join");

    publish(&owner, "tt", "echo", ECHO()).await;
    let shared = extract_structured(
        &owner
            .tools_call("host.tool_share", json!({"name": "tt", "visibility": "org"}))
            .await
            .expect("owner shares t as org"),
    );
    assert_eq!(shared["visibility"], json!("org"), "{shared}");
    assert_eq!(shared["members_reached"], json!(1), "{shared}");
    assert_eq!(shared["members"], json!([member_ns]), "{shared}");

    let out = extract_structured(&member.tools_call("org.tt", json!({"msg": "hi"})).await.expect("member calls org.t"));
    assert_eq!(out, json!({"msg": "hi"}));

    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let (run_ns, caller_ns): (String, String) = conn
        .query_row(
            "SELECT ot.namespace, ct.namespace FROM calls c \
             JOIN tenants ot ON ot.id = c.tenant_id JOIN tenants ct ON ct.id = c.caller_tenant_id \
             WHERE c.tool_name = 'tt' ORDER BY c.id DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("calls row");
    assert_eq!(run_ns, owner_ns, "the call ran under the owner's tenant");
    assert_eq!(caller_ns, member_ns, "the member is the recorded caller");
    let tools: i64 = conn.query_row("SELECT COUNT(*) FROM tools WHERE name = 'tt'", [], |r| r.get(0)).unwrap();
    assert_eq!(tools, 1, "no second tools row, no per-member copy");
    let member_tools: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tools x JOIN tenants t ON t.id = x.tenant_id WHERE t.namespace = ?1",
            [&member_ns],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(member_tools, 0, "the member owns no tool");
}
