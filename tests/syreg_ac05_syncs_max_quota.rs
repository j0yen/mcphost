//! PRD-mcphost-sync-source-registry-and-create AC5 (P0) -- Given a free
//! tenant with 5 syncs, When a sixth is created, Then `quota_syncs_max
//! {limit: 5, used: 5}` is returned and `host.quickstart.limits.plan.syncs_max`
//! reads 5 from the same `Plan` field.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn syreg_ac05() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Syreg AC5").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let syncs_max = server.state.plans.get("free").expect("free plan").syncs_max;
    assert_eq!(syncs_max, 5);

    for n in 0..syncs_max {
        client
            .tools_call(
                "host.sync.create",
                json!({"source": {"kind": "csv_url", "ref": format!("https://example.com/{n}.csv")}, "table": format!("t{n}")}),
            )
            .await
            .unwrap_or_else(|e| panic!("sync {n} is within the quota: {e:?}"));
    }

    let sixth = client
        .tools_call(
            "host.sync.create",
            json!({"source": {"kind": "csv_url", "ref": "https://example.com/6.csv"}, "table": "t6"}),
        )
        .await
        .expect_err("the sixth sync is over the free quota");
    assert_eq!(sixth.error_code.as_deref(), Some("quota_syncs_max"), "{sixth:?}");
    assert_eq!(sixth.data["limit"], json!(5), "{sixth:?}");
    assert_eq!(sixth.data["used"], json!(5), "{sixth:?}");

    // The refused call stored nothing.
    let listed = extract_structured(&client.tools_call("host.sync.list", json!({})).await.expect("list"));
    assert_eq!(listed["syncs"].as_array().map(Vec::len), Some(5), "{listed}");

    // The same Plan field is what host.quickstart reports.
    let quickstart = extract_structured(&client.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("quickstart"));
    assert_eq!(quickstart["limits"]["plan"]["syncs_max"], json!(syncs_max), "{quickstart}");
}
