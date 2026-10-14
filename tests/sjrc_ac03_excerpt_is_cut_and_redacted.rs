//! PRD-mcphost-scheduled-job-receipts AC3 (P0) — Given a run whose result is
//! 1 000 chars and contains a value matching `(?i)secret`, When the receipt
//! is written, Then `output_excerpt` is 240 chars plus `…`, the secret value
//! is `•••`, and `run_id` resolves through `host.runs.part`.

use crate::common;
use crate::sjrc;

use common::{McpClient, TestServer, extract_structured, signup_and_make_pro};
use serde_json::json;
use sjrc::fire_until_receipts;

#[tokio::test]
async fn long_result_is_cut_to_240_chars_with_secrets_masked() {
    let server = TestServer::start().await;
    let (_ns, key, _id) = signup_and_make_pro(&server, "SJRC AC3 Tenant", "cus_sjrc3").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.secret_set", json!({"name": "vault-pw", "value": "s3cr3t-value-xyz"}))
        .await
        .expect("secret_set");
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echoer", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    let set = extract_structured(
        &client
            .tools_call(
                "host.trigger.set",
                json!({
                    "tool": "echoer",
                    "schedule": "*/5 * * * *",
                    "args": {
                        "api_secret": "hunter2-plain",
                        "note": "s3cr3t-value-xyz",
                        "zpad": "x".repeat(1000),
                    },
                }),
            )
            .await
            .expect("trigger.set"),
    );
    let trigger_id = set["id"].as_str().expect("id").to_string();

    let got = fire_until_receipts(&client, &trigger_id, 1).await;
    let row = &got["rows"][0];
    let excerpt = row["output_excerpt"].as_str().expect("output_excerpt");
    assert_eq!(excerpt.chars().count(), 241, "240 chars plus the ellipsis: {excerpt}");
    assert!(excerpt.ends_with('…'), "{excerpt}");
    assert!(excerpt.contains("•••"), "{excerpt}");
    assert!(!excerpt.contains("hunter2-plain"), "key-named secret leaked: {excerpt}");
    assert!(!excerpt.contains("s3cr3t-value-xyz"), "tenant secret value leaked: {excerpt}");

    let run_id = row["run_id"].as_str().expect("run_id");
    let part = extract_structured(
        &client
            .tools_call("host.runs.part", json!({"run_id": run_id, "n": 0}))
            .await
            .expect("runs.part"),
    );
    assert_eq!(part["n"], json!(0), "{part:?}");
    assert!(part["data"].is_object(), "run_id must resolve to the full result: {part:?}");
}
