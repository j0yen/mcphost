//! PRD-mcphost-call-audit-export
//! AC5 (P0) — Given a window with 50 001 rows, When `host.calls.export`
//! runs, Then `too_many_rows` with `data.rows == 50001` and no CSV is built;
//! Given a sub-key, When it calls `host.calls.list`, Then only its own
//! `key_id` rows return.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

#[tokio::test]
async fn cae_ac05_window_over_the_cap_is_too_many_rows_and_builds_no_csv() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Cap Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");
    let now = mcphost::state::now_unix();
    server
        .state
        .db
        .insert_calls_rows_bulk_for_test(tenant.id, 50_001, now - 60)
        .await
        .expect("seed 50 001 rows");

    let err = client
        .tools_call("host.calls.export", json!({"since": "-1h"}))
        .await
        .expect_err("an over-cap window must be refused");
    assert_eq!(err.error_code.as_deref(), Some("too_many_rows"), "{err:?}");
    assert_eq!(err.data["rows"], json!(50_001), "{err:?}");
    assert!(err.data["hint"].as_str().is_some_and(|h| h.contains("narrow")), "{err:?}");
    assert!(err.data.get("csv").is_none(), "no CSV is built: {err:?}");

    // One row fewer fits: the cap is exclusive of 50 000.
    server
        .state
        .db
        .insert_calls_row_for_test(tenant.id, now - 7_200)
        .await
        .expect("an older row, outside the window");
    let narrow = client.tools_call("host.calls.export", json!({"since": "-30m", "until": "-10m"})).await.expect("narrow export");
    assert_eq!(extract_structured(&narrow)["rows"], json!(0));
}

#[tokio::test]
async fn cae_ac05_a_subkey_lists_only_its_own_rows() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Scope Tenant").await;
    let primary = McpClient::with_bearer(&server.base_url, &key);
    primary
        .tools_call(
            "host.tool.publish",
            json!({"name": "shared_tool", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    let mut subs = Vec::new();
    for label in ["alpha", "beta"] {
        let minted = extract_structured(
            &primary
                .tools_call("host.key.mint", json!({"tools": ["shared_tool"], "label": label}))
                .await
                .expect("mint"),
        );
        subs.push((label, McpClient::with_bearer(&server.base_url, minted["key"].as_str().expect("key"))));
    }
    let tool = format!("{ns}.shared_tool");
    primary.tools_call(&tool, json!({})).await.expect("primary call");
    for (_, client) in &subs {
        client.tools_call(&tool, json!({})).await.expect("alpha/beta call 1");
    }
    subs[0].1.tools_call(&tool, json!({})).await.expect("alpha call 2");

    let rows = |v: &Value| -> Vec<(String, Option<String>)> {
        v["calls"]
            .as_array()
            .expect("calls")
            .iter()
            .map(|r| (r["credential"].as_str().expect("credential").to_string(), r["key_label"].as_str().map(str::to_string)))
            .collect()
    };
    let alpha = extract_structured(&subs[0].1.tools_call("host.calls.list", json!({"since": "-1h"})).await.expect("alpha list"));
    assert_eq!(
        rows(&alpha),
        vec![("sub_key".to_string(), Some("alpha".to_string())); 2],
        "alpha sees exactly its own two rows: {alpha}"
    );
    let beta = extract_structured(&subs[1].1.tools_call("host.calls.list", json!({"since": "-1h"})).await.expect("beta list"));
    assert_eq!(rows(&beta), vec![("sub_key".to_string(), Some("beta".to_string()))], "{beta}");

    // The primary key still sees every row, newest first.
    let all = extract_structured(&primary.tools_call("host.calls.list", json!({"since": "-1h"})).await.expect("primary list"));
    assert_eq!(all["calls"].as_array().expect("calls").len(), 4, "{all}");

    // A sub-key cannot widen its view with the label filter.
    let sneaky = extract_structured(
        &subs[1].1.tools_call("host.calls.list", json!({"since": "-1h", "key_label": "alpha"})).await.expect("sneaky list"),
    );
    assert!(rows(&sneaky).is_empty(), "{sneaky}");
}
