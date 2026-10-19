//! PRD-mcphost-email-send-live-proof
//! AC3 (P0) -- Given an unverified tenant and the admin key, When
//! `admin.tenant_verify_owner {tenant, email}` runs, Then `owner_email`,
//! `owner_verified_at` and `owner_method = "admin"` are set by the same
//! single-winner UPDATE `verify_claim_code` uses, one `tenant_audit` row of
//! kind `claim.admin` exists whose actor equals the admin key id, and
//! `host.whoami` shows `owner_verified: true` and `owner.method == "admin"`;
//! the same call again is `already_verified {method: "admin"}`; a tenant
//! already verified by email/github/dns is `owner_verified_by_other_method`
//! and nothing changes; a tenant key, a sub-key or no key gets the same answer
//! every `admin.*` verb gives; a malformed `email` is `args_invalid`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

const OWNER: &str = "owner+probe@mcphost.dev";

/// `(owner_email, owner_verified_at, owner_method, [(subject, action)])` straight from SQLite.
fn snapshot(server: &TestServer, ns: &str) -> (Option<String>, Option<i64>, Option<String>, Vec<(String, String)>) {
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    let (id, email, at, method): (i64, Option<String>, Option<i64>, Option<String>) = conn
        .query_row(
            "SELECT id, owner_email, owner_verified_at, owner_method FROM tenants WHERE namespace = ?1",
            [ns],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .expect("tenant row");
    let mut stmt = conn
        .prepare("SELECT subject, action FROM tenant_audit WHERE tenant_id = ?1 AND action LIKE 'claim.%' ORDER BY id")
        .expect("prepare");
    let audit = stmt
        .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("rows");
    (email, at, method, audit)
}

#[tokio::test]
async fn emslp_ac03_admin_verifies_once_with_one_audit_row_and_whoami_says_admin() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "emslp ac3 happy").await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let tenant = McpClient::with_bearer(&server.base_url, &key);
    assert_eq!(snapshot(&server, &ns).2, None);

    let out = extract_structured(
        &admin.tools_call("admin.tenant_verify_owner", json!({"tenant": ns, "email": OWNER})).await.expect("verify"),
    );
    assert_eq!(out["method"], "admin", "{out}");
    assert_eq!(out["tenant"], json!(ns));
    assert_eq!(out["email"], OWNER);
    assert!(out["verified_at"].as_i64().is_some(), "{out}");

    let (email, at, method, audit) = snapshot(&server, &ns);
    assert_eq!(email.as_deref(), Some(OWNER));
    assert_eq!(at, out["verified_at"].as_i64());
    assert_eq!(method.as_deref(), Some("admin"));
    let admin_key_id = mcphost::auth::hash_key(ADMIN_KEY);
    assert_eq!(audit, vec![(admin_key_id, "claim.admin".to_string())], "exactly one claim.admin row, actor = admin key id");

    let who = extract_structured(&tenant.tools_call("host.whoami", json!({})).await.expect("whoami"));
    assert_eq!(who["owner_verified"], json!(true), "{who}");
    assert_eq!(who["owner"]["method"], "admin", "{who}");

    // Again: already_verified {method: admin}, nothing changes.
    let before = snapshot(&server, &ns);
    let err = admin
        .tools_call("admin.tenant_verify_owner", json!({"tenant": ns, "email": "other@mcphost.dev"}))
        .await
        .expect_err("second call refuses");
    assert_eq!(err.error_code.as_deref(), Some("already_verified"), "{}", err.message);
    assert_eq!(err.data["method"], "admin");
    assert_eq!(snapshot(&server, &ns), before);
}

#[tokio::test]
async fn emslp_ac03_a_tenant_verified_by_another_method_is_never_overwritten() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    for method in ["email", "github", "dns"] {
        let (ns, _key) = signup(&server.base_url, &format!("emslp ac3 {method}")).await;
        let row = server.state.db.find_tenant_by_namespace(ns.clone()).await.unwrap().unwrap();
        server.state.db.verify_owner(row.id, method.to_string(), format!("{method}-handle")).await.expect("verify");
        let before = snapshot(&server, &ns);

        let err = admin
            .tools_call("admin.tenant_verify_owner", json!({"tenant": ns, "email": OWNER}))
            .await
            .expect_err("refused");
        assert_eq!(err.error_code.as_deref(), Some("owner_verified_by_other_method"), "{method}: {}", err.message);
        assert_eq!(err.data["method"], method);
        assert_eq!(err.data["verified_at"].as_i64(), before.1, "{method}");
        assert_eq!(snapshot(&server, &ns), before, "{method}: no column or audit row changes");
    }
}

#[tokio::test]
async fn emslp_ac03_only_the_admin_key_reaches_the_verb_and_a_bad_email_is_args_invalid() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "emslp ac3 gate").await;
    let tenant = McpClient::with_bearer(&server.base_url, &key);
    tenant
        .tools_call("host.tool.publish", json!({"name": "probe_tool", "kind": "echo", "spec": {"schema": {"type": "object"}}}))
        .await
        .expect("publish");
    let sub = extract_structured(
        &tenant.tools_call("host.key.mint", json!({"tools": ["probe_tool"], "label": "emslp"})).await.expect("mint"),
    );
    let sub = McpClient::with_bearer(&server.base_url, sub["key"].as_str().expect("sub key"));
    let anon = McpClient::new(&server.base_url);
    let args = json!({"tenant": ns, "email": OWNER});
    let reference = json!({"tenant": ns, "domain_namespace": "example.com"});
    let before = snapshot(&server, &ns);

    for (who, client) in [("tenant key", &tenant), ("sub-key", &sub), ("no key", &anon)] {
        let got = client.tools_call("admin.tenant_verify_owner", args.clone()).await.expect_err(who);
        let want = client.tools_call("admin.tenant_verify_namespace", reference.clone()).await.expect_err(who);
        assert_eq!(got.error_code, want.error_code, "{who}: same answer as every admin.* verb");
        assert_eq!((got.code, &got.message), (want.code, &want.message), "{who}");
        assert_ne!(got.error_code.as_deref(), Some("owner_verified_by_other_method"));
    }
    assert_eq!(snapshot(&server, &ns), before, "nothing written");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    for bad in ["not-an-email", "a b@c.d", "x@nodot", ""] {
        let err = admin
            .tools_call("admin.tenant_verify_owner", json!({"tenant": ns, "email": bad}))
            .await
            .expect_err("malformed email");
        assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{bad}: {}", err.message);
        assert!(err.message.contains("email"), "{bad}: {}", err.message);
    }
    assert_eq!(snapshot(&server, &ns), before, "nothing written");
}
