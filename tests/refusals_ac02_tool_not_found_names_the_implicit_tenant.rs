//! PRD-mcphost-refusals-name-their-cause AC2 -- Given tenant A published
//! `t_ac2` and keyless B became an implicit tenant under 600 s ago, When B
//! calls `tools/call t_ac2` and a name no tenant owns, Then both `-32002`
//! bodies carry `data.reason` `tool_not_found_on_implicit_tenant`,
//! `data.tenant` = B and the § Hint fields, keys a subset of the listed set,
//! no byte of A's distinctive namespace or any key; with A's `tenant_key`
//! `t_ac2` succeeds; B older than 600 s gets no `data.reason`.

use crate::common;

use common::{McpClient, RpcError, TestServer, extract_structured};
use mcphost::errors::TOOL_NOT_FOUND_FIELDS;
use serde_json::json;

fn assert_hint_body(err: &RpcError, tenant_b: &str, created_unix: i64, forbidden: &[&str]) {
    assert_eq!(err.code, -32002, "{err:?}");
    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"), "{err:?}");
    let data = err.data.as_object().expect("data is an object");
    assert_eq!(data["reason"], "tool_not_found_on_implicit_tenant", "{err:?}");
    assert_eq!(data["tenant"], tenant_b);
    assert_eq!(data["signup_source"], "implicit");
    assert_eq!(data["created_unix"], created_unix);
    assert_eq!(
        data["hint"],
        format!(
            "this connection is a new tenant made by implicit sign-up at {created_unix}; if you published this tool \
             under another tenant, pass that tenant's tenant_key (or Authorization: Bearer) on this call"
        )
    );
    for key in data.keys() {
        assert!(TOOL_NOT_FOUND_FIELDS.contains(&key.as_str()), "unlisted key {key} in {err:?}");
    }
    let wire = format!("{} {}", err.message, err.data);
    for needle in forbidden {
        assert!(!wire.contains(needle), "the body leaks {needle}: {wire}");
    }
}

#[tokio::test]
async fn tool_not_found_on_a_young_implicit_tenant_names_it_and_leaks_nothing() {
    let server = TestServer::start().await;

    let (ns_a, key_a) = common::signup(&server.base_url, "refusals ac2 tenant A").await;
    let a = McpClient::with_bearer(&server.base_url, &key_a);
    let qualified = common::publish(&a, "t_ac2", "echo", json!({"schema": {"type": "object"}})).await;
    assert_eq!(qualified, format!("{ns_a}.t_ac2"));

    // B: keyless, its first host.* call makes it an implicit tenant; it then
    // reconnects over the personal URL that call returned (`onboarding.url`).
    let first = McpClient::new(&server.base_url).with_session_continuity();
    let whoami = extract_structured(&first.tools_call("host.whoami", json!({})).await.expect("B whoami"));
    let ns_b = whoami["onboarding"]["tenant"].as_str().expect("B's namespace").to_string();
    let url = whoami["onboarding"]["url"].as_str().expect("B's personal url");
    let b = McpClient::new(&server.base_url).with_path(url.strip_prefix(&server.base_url).expect("url on this server"));
    assert_ne!(ns_a, ns_b);
    let created_unix = server
        .state
        .db
        .list_tenants()
        .await
        .expect("tenants")
        .into_iter()
        .find(|t| t.namespace == ns_b)
        .and_then(|t| t.created_unix)
        .expect("B's created_unix");

    // The distinctive strings must be able to fail the check: neither is part
    // of the hint text or of a key name.
    let forbidden = [ns_a.as_str(), key_a.as_str()];
    for needle in forbidden {
        assert!(!mcphost::errors::implicit_tenant_hint(created_unix).contains(needle));
        assert!(!TOOL_NOT_FOUND_FIELDS.iter().any(|k| k.contains(needle)));
    }

    // A name another tenant owns (locally) and a name no tenant owns: the same body shape.
    let owned_elsewhere = b.tools_call("t_ac2", json!({})).await.expect_err("B has no t_ac2");
    let owned_by_nobody = b.tools_call("t_ac2_nobody_owns_this", json!({})).await.expect_err("nobody owns it");
    assert_hint_body(&owned_elsewhere, &ns_b, created_unix, &forbidden);
    assert_hint_body(&owned_by_nobody, &ns_b, created_unix, &forbidden);
    let keys = |e: &RpcError| -> Vec<String> { e.data.as_object().unwrap().keys().cloned().collect() };
    assert_eq!(keys(&owned_elsewhere), keys(&owned_by_nobody), "no existence oracle");

    // A's own key: the tool exists and runs.
    a.tools_call(&qualified, json!({})).await.expect("A calls its own t_ac2");

    // B older than 600 s: today's -32002, no data.reason.
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    conn.execute("UPDATE tenants SET created_unix = ?1 WHERE namespace = ?2", rusqlite::params![created_unix - 601, ns_b])
        .expect("age B");
    drop(conn);
    let old = b.tools_call("t_ac2", json!({})).await.expect_err("still not found");
    assert_eq!(old.code, -32002);
    assert_eq!(old.error_code.as_deref(), Some("tool_not_found"));
    assert!(old.data.get("reason").is_none(), "{old:?}");
    assert!(old.data.get("tenant").is_none(), "{old:?}");
}
