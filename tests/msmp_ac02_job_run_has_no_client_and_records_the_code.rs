//! PRD-mcphost-sampling-from-tools
//! AC2 (P0) — Given a job-triggered run of the same tool, When it executes,
//! Then the bridge returns `sampling_unsupported {reason: "no_client"}` and
//! the run row records the code, not a traceback.

use crate::common;
use common::{McpClient, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

const TOOL: &str = "import mcphost\ndef main(args):\n    if args.get(\"warm\"):\n        return {\"warm\": True}\n    if args.get(\"catch\"):\n        try:\n            return mcphost.sampling.ask(\"Urgent?\", max_tokens=4)\n        except mcphost.sampling.SamplingError as e:\n            return {\"code\": e.code, \"reason\": e.data.get(\"reason\"), \"alternative\": e.data.get(\"alternative\")}\n    return mcphost.sampling.ask(\"Urgent?\", max_tokens=4)\n";

async fn finished_run(client: &McpClient, args: Value) -> Value {
    let enqueue = extract_structured(
        &client
            .tools_call("host.tool_call", json!({"name": "triage", "args": args, "async": true}))
            .await
            .expect("enqueue"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id").to_string();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let run = extract_structured(&client.tools_call("host.runs.get", json!({"run_id": run_id})).await.expect("runs.get"));
        if matches!(run["status"].as_str(), Some("done" | "error" | "timeout")) || Instant::now() > deadline {
            return run;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn a_job_run_cannot_sample_and_its_row_records_sampling_unsupported() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs.0)).await;
    let (ns, key) = signup(&server.base_url, "Msmp AC2").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.tool_publish", json!({"name": "triage", "kind": "python", "spec": {"source": TOOL}}))
        .await
        .expect("publish");
    poll_until_ready(&client, &format!("{ns}.triage"), json!({"warm": true}), Duration::from_secs(20))
        .await
        .expect("warm");

    // The refusal as the tool sees it: code, reason and the alternative.
    let caught = finished_run(&client, json!({"catch": true})).await;
    assert_eq!(caught["status"], json!("done"), "{caught}");
    let result = &caught["result"];
    assert_eq!(result["code"], json!("sampling_unsupported"), "{caught}");
    assert_eq!(result["reason"], json!("no_client"), "{caught}");
    assert!(result["alternative"].as_str().is_some_and(|a| a.contains("prompt-kind")), "{caught}");

    // Uncaught: the run row itself records the code, not a traceback.
    let failed = finished_run(&client, json!({})).await;
    assert_eq!(failed["status"], json!("error"), "{failed}");
    assert_eq!(failed["error_class"], json!("sampling_unsupported"), "{failed}");
    assert!(!failed.to_string().contains("Traceback"), "the row carries the code, not a traceback: {failed}");
}
