//! PRD-mcphost-org-export-and-offboard-v2
//! AC4 — Given the owner, When it calls `host.org.offboard {confirm: "wrong"}`,
//! Then `confirm_mismatch` and every member stays enabled; with `confirm`
//! equal to the org name, Then every live member, the owner included, is
//! `disabled = 1, disabled_reason = org_offboard`, each member's next call
//! answers `tenant_disabled` with that reason, and a repeat call answers
//! `already_offboarded`.

use crate::common;
use common::orgx::fixture_org;
use common::{TestServer, extract_structured};
use serde_json::json;

fn disabled_rows(server: &TestServer) -> Vec<(String, i64, Option<String>)> {
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let mut stmt = conn
        .prepare(
            "SELECT t.namespace, t.disabled, t.disabled_reason FROM tenants t \
             JOIN org_members m ON m.tenant_id = t.id WHERE m.removed_unix IS NULL ORDER BY t.id",
        )
        .expect("prepare");
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).expect("query").collect::<Result<_, _>>().expect("rows")
}

#[tokio::test]
async fn orgx_ac04() {
    let server = TestServer::start().await;
    let fx = fixture_org(&server, "acme", 2, 1).await;

    // A wrong or missing confirm changes nothing.
    let err = fx.owner.tools_call("host.org.offboard", json!({"confirm": "wrong"})).await.expect_err("wrong confirm");
    assert_eq!(err.error_code.as_deref(), Some("confirm_mismatch"), "{err:?}");
    let err = fx.owner.tools_call("host.org.offboard", json!({})).await.expect_err("missing confirm");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    assert!(disabled_rows(&server).iter().all(|(_, disabled, reason)| *disabled == 0 && reason.is_none()), "every member stays enabled");

    // An org export that is still running refuses the offboard and changes nothing.
    let owner = server.state.db.find_tenant_by_namespace(fx.owner_ns.clone()).await.expect("find").expect("owner");
    server
        .state
        .db
        .start_export_run(owner.id, "01ORGXAC04RUNNINGEXPORT00".to_string(), mcphost::export::ORG_EXPORT_TOOL_NAME.to_string(), 300, "{}".to_string())
        .await
        .expect("start run");
    let err = fx.owner.tools_call("host.org.offboard", json!({"confirm": "acme"})).await.expect_err("export running");
    assert_eq!(err.error_code.as_deref(), Some("org_export_running"), "{err:?}");
    assert_eq!(err.data["run_id"], json!("01ORGXAC04RUNNINGEXPORT00"), "{:?}", err.data);
    assert!(disabled_rows(&server).iter().all(|(_, disabled, _)| *disabled == 0));
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    conn.execute("UPDATE runs SET status = 'done' WHERE id = '01ORGXAC04RUNNINGEXPORT00'", []).expect("finish run");

    // The right confirm switches all three off in one call.
    let out = extract_structured(&fx.owner.tools_call("host.org.offboard", json!({"confirm": "acme"})).await.expect("offboard"));
    assert_eq!(out["org"], "acme");
    assert_eq!(out["members_disabled"], 3, "{out}");
    assert!(out["offboarded_unix"].as_i64().is_some_and(|t| t > 0), "{out}");
    assert_eq!(out["billing_canceled"], json!([]));
    let rows = disabled_rows(&server);
    assert_eq!(rows.len(), 3);
    for (ns, disabled, reason) in &rows {
        assert_eq!((*disabled, reason.as_deref()), (1, Some("org_offboard")), "{ns}");
    }
    let audited: i64 = conn.query_row("SELECT COUNT(DISTINCT tenant_id) FROM tenant_audit WHERE action = 'org.offboard'", [], |r| r.get(0)).expect("audit");
    assert_eq!(audited, 3, "one org.offboard audit row per member");

    // Each member's next call, the owner's included, answers tenant_disabled with the reason.
    for (ns, client) in fx.members.iter().map(|(ns, c)| (ns.as_str(), c)).chain([(fx.owner_ns.as_str(), &fx.owner)]) {
        let err = client.tools_call("host.whoami", json!({})).await.expect_err(ns);
        assert_eq!(err.error_code.as_deref(), Some("tenant_disabled"), "{ns}: {err:?}");
        assert_eq!(err.data["disabled_reason"], json!("org_offboard"), "{ns}: {:?}", err.data);
    }

    // A repeat call by the (disabled) owner answers already_offboarded and flips nothing more.
    let again = extract_structured(&fx.owner.tools_call("host.org.offboard", json!({"confirm": "acme"})).await.expect("repeat"));
    assert_eq!(again["already_offboarded"], json!(true), "{again}");
    assert_eq!(again["offboarded_unix"], out["offboarded_unix"]);
    let audited: i64 = conn.query_row("SELECT COUNT(*) FROM tenant_audit WHERE action = 'org.offboard'", [], |r| r.get(0)).expect("audit");
    assert_eq!(audited, 3, "the repeat wrote nothing");
}
