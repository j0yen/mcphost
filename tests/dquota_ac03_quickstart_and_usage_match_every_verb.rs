//! PRD-mcphost-data-quota-in-every-response
//! AC3 -- Given the same tenant, When `host.quickstart` (keyed) and
//! `host.usage` run, Then `limits.quota` and `usage.quota` are
//! byte-identical to each other and to the union of every data verb's
//! `data.quota` over one call each.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Map, Value, json};

/// One call of each data-verb family; returns the union of their
/// `data.quota` objects. `host.export` goes last: it starts a background job.
pub async fn union_of_verb_quotas(client: &McpClient) -> Value {
    let calls: [(&str, Value); 5] = [
        ("host.state.list", json!({})),
        ("host.table.list", json!({})),
        ("host.docs.status", json!({})),
        ("host.blob.list", json!({})),
        ("host.export", json!({})),
    ];
    let mut union = Map::new();
    for (tool, args) in calls {
        let body = extract_structured(&client.tools_call(tool, args).await.unwrap_or_else(|e| panic!("{tool}: {e:?}")));
        let quota = body["data"]["quota"]
            .as_object()
            .unwrap_or_else(|| panic!("{tool} carried no data.quota: {body:?}"));
        for (k, v) in quota {
            union.insert(k.clone(), v.clone());
        }
    }
    Value::Object(union)
}

#[tokio::test]
async fn quickstart_usage_and_the_verb_union_are_one_object() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Quota").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.table.create", json!({"name": "t", "columns": {"n": "integer"}}))
        .await
        .expect("create");
    client
        .tools_call("host.table.append", json!({"table": "t", "rows": [{"n": 1}, {"n": 2}]}))
        .await
        .expect("append");
    client.tools_call("host.state.set", json!({"key": "k", "value": "v"})).await.expect("set");

    let quickstart = extract_structured(&client.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("quickstart"));
    let usage = extract_structured(&client.tools_call("host.usage", json!({})).await.expect("usage"));
    let from_quickstart = &quickstart["limits"]["quota"];
    let from_usage = &usage["quota"];
    let union = union_of_verb_quotas(&client).await;

    assert!(from_quickstart.as_object().is_some_and(|o| !o.is_empty()), "{quickstart:?}");
    assert_eq!(from_quickstart.to_string(), from_usage.to_string(), "quickstart vs usage");
    assert_eq!(from_usage.to_string(), union.to_string(), "usage vs union of verbs");
    assert_eq!(from_usage["table_rows_max"]["used"], json!(2));
}
