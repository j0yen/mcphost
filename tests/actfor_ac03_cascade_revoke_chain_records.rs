//! PRD-mcphost-acting-for-in-ledger-and-audit
//! AC3 (P0) — Given a cascade revoke of 3 keys, When `host.audit.chain
//! {subject: "key:lead-job-42"}` runs, Then it lists 3 `revoke_cascade`
//! records each with `detail.revoked_by == "lead-job-42"` and
//! `host.audit.verify` returns `ok: true`.

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
async fn actfor_ac03_cascade_revoke_writes_one_chained_record_per_key() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "ActFor Cascade Tenant").await;
    let primary = McpClient::with_bearer(&server.base_url, &key);
    let lead = delegate(&server.base_url, &primary, "lead-job-42", 3000).await;
    let tester = delegate(&server.base_url, &lead, "tester", 2000).await;
    let _deployer = delegate(&server.base_url, &tester, "deployer", 1000).await;

    let revoked = extract_structured(
        &primary
            .tools_call("host.key.revoke", json!({"label": "lead-job-42", "cascade": true}))
            .await
            .expect("cascade revoke"),
    );
    assert_eq!(revoked["revoked"], json!(3), "{revoked}");

    let chain = extract_structured(
        &primary
            .tools_call("host.audit.chain", json!({"subject": "key:lead-job-42"}))
            .await
            .expect("audit chain"),
    );
    let records = chain["records"].as_array().expect("records");
    assert_eq!(records.len(), 3, "{chain}");
    for r in records {
        assert_eq!(r["action"], json!("revoke_cascade"), "{r}");
        assert_eq!(r["detail"]["revoked_by"], json!("lead-job-42"), "{r}");
    }
    let mut labels: Vec<&str> = records.iter().map(|r| r["detail"]["label"].as_str().expect("label")).collect();
    labels.sort_unstable();
    assert_eq!(labels, ["deployer", "lead-job-42", "tester"], "{chain}");

    let ids: Vec<i64> = records.iter().map(|r| r["id"].as_i64().expect("id")).collect();
    let verify = extract_structured(
        &primary
            .tools_call(
                "host.audit.verify",
                json!({"from_id": ids.iter().min().unwrap(), "to_id": ids.iter().max().unwrap()}),
            )
            .await
            .expect("audit verify"),
    );
    assert_eq!(verify["ok"], json!(true), "{verify}");
}
