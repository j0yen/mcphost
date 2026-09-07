//! PRD-mcphost-tool-test AC2 — Given a `python` source that raises on one
//! invocation, When tested, Then that invocation returns `ok: false` with
//! the exception type and a bounded traceback excerpt, the other invocation
//! still runs, and the JSON-RPC call as a whole succeeds.

mod common;
use common::{
    McpClient, TempDataDir, TestServer, extract_structured, poll_spec_test_until_ready,
    python_kind_registry, signup,
};
use serde_json::json;
use std::time::Duration;

fn flaky_spec() -> serde_json::Value {
    json!({
        "source": "def main(args):\n    if args.get(\"boom\"):\n        raise ValueError(\"kaboom\")\n    return {\"ok\": True}\n",
    })
}

#[tokio::test]
async fn a_raising_invocation_reports_ok_false_without_failing_the_call() {
    let data_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&data_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Flaky Test Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let result = poll_spec_test_until_ready(
        &client,
        json!({
            "kind": "python",
            "spec": flaky_spec(),
            "invocations": [{"boom": true}, {"boom": false}],
        }),
        Duration::from_secs(30),
    )
    .await;
    let structured = extract_structured(&result);
    let invocations = structured["invocations"].as_array().expect("array");
    assert_eq!(invocations.len(), 2);

    assert_eq!(invocations[0]["ok"], json!(false));
    assert_eq!(invocations[0]["exception_class"], json!("ValueError"));
    let traceback = invocations[0]["traceback"]
        .as_str()
        .expect("traceback string");
    assert!(!traceback.is_empty());
    assert!(
        traceback.len() <= 8 * 1024,
        "traceback must be bounded to 8 KiB, got {} bytes",
        traceback.len()
    );

    assert_eq!(invocations[1]["ok"], json!(true));
    assert_eq!(invocations[1]["output"]["result"]["ok"], json!(true));
}
