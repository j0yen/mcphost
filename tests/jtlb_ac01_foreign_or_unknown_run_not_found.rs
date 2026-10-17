//! PRD-mcphost-job-timeline-tool-b AC1 (P0) -- Given a run id owned by another
//! tenant or unknown, When the timeline is read, Then the error is
//! `run_not_found`, identical to `host.runs.get`.

use crate::common;
use common::{TestServer, chain_kind_registry, extract_structured, publish, signup};
use serde_json::json;

/// The error's Debug text with the per-request `request_id` blanked.
fn rendered(err: impl std::fmt::Debug) -> String {
    let text = format!("{err:?}");
    match text.find("\"request_id\": String(\"") {
        Some(at) => {
            let start = at + "\"request_id\": String(\"".len();
            let end = start + text[start..].find('"').unwrap_or(0);
            format!("{}{}", &text[..start], &text[end..])
        }
        None => text,
    }
}

#[tokio::test]
async fn jtlb_ac01() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "JTLB1 Owner").await;
    let owner = common::McpClient::with_bearer(&server.base_url, &key);
    let (_ns2, key2) = signup(&server.base_url, "JTLB1 Other").await;
    let other = common::McpClient::with_bearer(&server.base_url, &key2);

    let tool = publish(&owner, "solo", "echo", json!({"schema": {"type": "object"}})).await;
    owner.tools_call(&tool, json!({"n": 1})).await.expect("call");
    let runs = extract_structured(&owner.tools_call("host.runs.list", json!({})).await.expect("runs.list"));
    let run_id = runs["runs"][0]["run_id"].as_str().expect("run id").to_string();

    // The owner reads it fine through both verbs.
    owner.tools_call("host.job.timeline", json!({"run_id": run_id})).await.expect("owner timeline");
    owner.tools_call("host.runs.get", json!({"run_id": run_id})).await.expect("owner runs.get");

    for (who, client, id) in [("foreign", &other, run_id.as_str()), ("unknown", &owner, "run_does_not_exist")] {
        let timeline = rendered(
            client.tools_call("host.job.timeline", json!({"run_id": id})).await.expect_err("timeline refuses"),
        );
        let get = rendered(client.tools_call("host.runs.get", json!({"run_id": id})).await.expect_err("get refuses"));
        assert!(timeline.contains("run_not_found"), "{who}: {timeline}");
        assert!(get.contains("run_not_found"), "{who}: {get}");
        // Identical error: same code, message and data once the per-request id is blanked.
        assert_eq!(timeline, get, "{who}: timeline error differs from host.runs.get");
    }
}
