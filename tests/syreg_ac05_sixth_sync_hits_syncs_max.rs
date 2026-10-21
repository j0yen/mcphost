//! PRD-mcphost-sync-source-registry-and-create AC5 -- Given a free tenant
//! with 5 syncs, When a sixth is created, Then `quota_syncs_max {limit: 5,
//! used: 5}` is returned and `host.quickstart.limits.plan.syncs_max` reads 5
//! from the same `Plan` field.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn syreg_ac05_sixth_sync_is_quota_syncs_max_and_quickstart_agrees() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SYREG AC5").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    for i in 0..5 {
        client
            .tools_call(
                "host.sync.create",
                json!({"source": {"kind": "csv_url", "ref": format!("https://data.example.com/{i}.csv")}, "table": format!("t{i}")}),
            )
            .await
            .unwrap_or_else(|e| panic!("sync {i} is under the limit: {e:?}"));
    }
    let err = client
        .tools_call(
            "host.sync.create",
            json!({"source": {"kind": "csv_url", "ref": "https://data.example.com/6.csv"}, "table": "t6"}),
        )
        .await
        .expect_err("sixth sync is over syncs_max");
    assert_eq!(err.error_code.as_deref(), Some("quota_syncs_max"), "{err:?}");
    assert_eq!(err.data["limit"], 5, "{:?}", err.data);
    assert_eq!(err.data["used"], 5, "{:?}", err.data);

    let listed = extract_structured(&client.tools_call("host.sync.list", json!({})).await.expect("list"));
    assert_eq!(listed["syncs"].as_array().expect("syncs").len(), 5, "no sixth row was written");

    let quickstart = extract_structured(&client.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("quickstart"));
    let from_struct = server.state.plans.get("free").expect("free plan").syncs_max;
    assert_eq!(quickstart["limits"]["plan"]["syncs_max"], json!(5), "{quickstart}");
    assert_eq!(quickstart["limits"]["plan"]["syncs_max"], json!(from_struct));
}
