//! PRD-mcphost-python-dependency-policy
//! AC8 (P1) — Given a new advisory for a stored lock, When the daily
//! re-audit runs, Then `host.tool_list` shows `advisories: 1` for that
//! tool and the tool still runs.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn reaudit_flags_a_newly_discovered_advisory_without_disabling_the_tool() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC8 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // A pin the embedded advisory table (AC3's `urllib3==1.26.4`) does NOT
    // already flag, so this tool's lock starts clean at publish time --
    // requirement 6's "new" advisory has to come from a later database
    // refresh, not from publish-time knowledge.
    let spec = json!({
        "source": "import idna\ndef main(args):\n    return {\"version\": idna.__version__}\n",
        "requirements": ["idna==3.6"],
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "reaudited", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let listed_before = extract_structured(
        &client.tools_call("host.tool_list", json!({})).await.expect("tool_list"),
    );
    let before = listed_before["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"] == json!(format!("{ns}.reaudited")))
        .unwrap_or_else(|| panic!("tool not listed: {listed_before:?}"));
    assert_eq!(before["advisories"], json!(0), "{before:?}");

    // requirement 6: "an offline database refreshed daily" -- simulate a
    // day's refresh finding a new advisory for the exact package/version
    // already locked, via `$MCPHOST_ADVISORY_DB_PATH` (see
    // `deps::advisory_db`'s doc comment for why this, not the network, is
    // this crate's own seam for that refresh).
    let advisory_db_path = envs_dir.0.join("advisory-db.json");
    std::fs::write(
        &advisory_db_path,
        json!([{"package": "idna", "version": "3.6", "id": "GHSA-TEST-0001", "fixed": "3.7"}])
            .to_string(),
    )
    .expect("write fixture advisory db");
    // SAFETY: this file has exactly one #[tokio::test] fn, so no sibling
    // test in this binary can observe (or race) this process-wide env var.
    unsafe {
        std::env::set_var("MCPHOST_ADVISORY_DB_PATH", &advisory_db_path);
    }

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let reaudit = extract_structured(
        &admin
            .tools_call("admin.dependency_reaudit", json!({}))
            .await
            .expect("admin.dependency_reaudit"),
    );

    unsafe {
        std::env::remove_var("MCPHOST_ADVISORY_DB_PATH");
    }

    assert_eq!(
        reaudit["tools_flagged"].as_i64(),
        Some(1),
        "{reaudit:?}"
    );

    let listed_after = extract_structured(
        &client.tools_call("host.tool_list", json!({})).await.expect("tool_list"),
    );
    let after = listed_after["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"] == json!(format!("{ns}.reaudited")))
        .unwrap_or_else(|| panic!("tool not listed: {listed_after:?}"));
    assert_eq!(after["advisories"], json!(1), "{after:?}");

    // requirement 6: "without disabling them" -- the tool must still run.
    let result = client
        .tools_call(&format!("{ns}.reaudited"), json!({}))
        .await
        .unwrap_or_else(|e| panic!("tool must still run after being flagged: {} {}", e.code, e.message));
    let structured = extract_structured(&result);
    assert!(
        structured["status"] == json!("building") || structured["version"] == json!("3.6"),
        "{structured:?}"
    );
}
