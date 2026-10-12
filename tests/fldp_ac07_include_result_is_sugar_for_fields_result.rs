//! PRD-mcphost-fields-projection-on-list-verbs
//! AC7 (P1) -- Given `runs.list(include_result: true)`, When called, Then the
//! result equals `runs.list(fields: [<all>, "result"])` and `_meta.deprecated`
//! names `include_result`.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use mcphost::paged;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

#[tokio::test]
async fn include_result_equals_fields_with_result_and_reports_the_deprecation() {
    let server = TestServer::start().await;
    let (_ns, key) = common::signup(&server.base_url, "Fldp AC7").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.tool_publish", json!({"name": "reflect", "kind": "echo", "spec": {"schema": {"type": "object"}}}))
        .await
        .expect("publish");
    let set = extract_structured(
        &client
            .tools_call(
                "host.trigger.set",
                json!({"tool": "reflect", "kind": "event", "verify": {"scheme": "none", "allow_unverified": true}}),
            )
            .await
            .expect("trigger.set"),
    );
    let tested = extract_structured(
        &client.tools_call("host.trigger.test", json!({"id": set["id"], "body": {"k": 1}})).await.expect("test"),
    );
    let run_id = tested["run_id"].as_str().expect("run_id").to_string();
    let deadline = Instant::now() + Duration::from_secs(5);
    let result = loop {
        let got = extract_structured(&client.tools_call("host.runs.get", json!({"run_id": run_id})).await.expect("get"));
        if got["status"] == json!("done") {
            break got["result"].clone();
        }
        assert!(Instant::now() < deadline, "run {run_id} never finished");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_ne!(result, Value::Null, "the comparison is only meaningful with an inlined result");

    let sugar = client.tools_call("host.runs.list", json!({"include_result": true})).await.expect("include_result");
    let mut all: Vec<&str> = paged::fields_for("host.runs.list").expect("runs fields").to_vec();
    all.push("result");
    let explicit = client.tools_call("host.runs.list", json!({"fields": all})).await.expect("fields");

    let sugar_body = extract_structured(&sugar);
    assert_eq!(sugar_body, extract_structured(&explicit));
    let row = sugar_body["runs"].as_array().expect("runs").iter().find(|r| r["run_id"] == json!(run_id)).expect("row");
    assert_eq!(row["result"], result, "{row}");

    let dep = &sugar["_meta"]["deprecated"];
    assert_eq!(dep["argument"], json!("include_result"), "{sugar:?}");
    assert!(dep["replaced_by"].as_str().is_some_and(|r| r.contains("fields")), "{dep}");
    assert!(explicit.get("_meta").is_none_or(|m| m.get("deprecated").is_none()), "{explicit:?}");
}
