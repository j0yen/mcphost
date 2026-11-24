//! PRD-mcphost-org-shared-tools-namespace-v2
//! AC7 — Given M publishes `t` and shares it as `org` while O's `org.t`
//! exists, When the share runs, Then `org_tool_name_taken {publisher:
//! O.namespace}`, M's `t` stays `private`, and two members sharing one fresh
//! name at once end with exactly one org tool and one `org_tool_name_taken`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, publish, signup};
use serde_json::{Value, json};

fn echo() -> Value {
    json!({"schema": {"type": "object"}})
}

#[tokio::test]
async fn orgs_ac07() {
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
    let (_m_ns, m_key) = signup(&server.base_url, "Member M").await;
    let m = McpClient::with_bearer(&server.base_url, &m_key);
    m.tools_call("host.org.join", json!({"code": code})).await.expect("M joins");
    let (_p_ns, p_key) = signup(&server.base_url, "Member P").await;
    let p = McpClient::with_bearer(&server.base_url, &p_key);
    p.tools_call("host.org.join", json!({"code": code})).await.expect("P joins");

    // O's org.tt exists; M's own `tt` cannot take the same org name.
    publish(&owner, "tt", "echo", echo()).await;
    owner.tools_call("host.tool_share", json!({"name": "tt", "visibility": "org"})).await.expect("O shares tt");
    publish(&m, "tt", "echo", echo()).await;
    let err = m
        .tools_call("host.tool_share", json!({"name": "tt", "visibility": "org"}))
        .await
        .expect_err("org.tt is taken");
    assert_eq!(err.error_code.as_deref(), Some("org_tool_name_taken"), "{err:?}");
    assert_eq!(err.data["publisher"], json!(owner_ns), "{:?}", err.data);

    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let m_visibility: String = conn
        .query_row(
            "SELECT t.visibility FROM tools t JOIN tenants te ON te.id = t.tenant_id \
             WHERE t.name = 'tt' AND te.namespace != ?1",
            [&owner_ns],
            |r| r.get(0),
        )
        .expect("M's tool row");
    assert_eq!(m_visibility, "private", "M's tt stays private");

    // Two members share one fresh name at once: one wins, one is refused.
    publish(&m, "zz", "echo", echo()).await;
    publish(&p, "zz", "echo", echo()).await;
    let (a, b) = tokio::join!(
        m.tools_call("host.tool_share", json!({"name": "zz", "visibility": "org"})),
        p.tools_call("host.tool_share", json!({"name": "zz", "visibility": "org"})),
    );
    let refused: Vec<_> = [&a, &b].into_iter().filter_map(|r| r.as_ref().err()).collect();
    assert_eq!(refused.len(), 1, "exactly one share is refused: {a:?} {b:?}");
    assert_eq!(refused[0].error_code.as_deref(), Some("org_tool_name_taken"), "{:?}", refused[0]);
    let org_zz: i64 = conn
        .query_row("SELECT COUNT(*) FROM tools WHERE name = 'zz' AND visibility = 'org'", [], |r| r.get(0))
        .expect("count");
    assert_eq!(org_zz, 1, "exactly one org tool named zz");
}
