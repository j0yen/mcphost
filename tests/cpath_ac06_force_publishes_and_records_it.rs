//! PRD-mcphost-chain-publish-time-path-check AC6 (P0) -- Given the failing
//! chain of AC1, When published with `force: true`, Then the tool exists and
//! the response `gates` entry reads `verdict: will_fail, forced: true`.

use crate::common;
use common::{McpClient, TestServer, chain_and_http_kind_registry, extract_structured, publish, signup};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// One async call of `tool`, polled to a terminal status (must be `done`).
async fn run_to_done(client: &McpClient, tool: &str, args: Value) {
    let enqueue = extract_structured(
        &client
            .tools_call("host.tool_call", json!({"name": tool, "args": args, "async": true}))
            .await
            .expect("enqueue ok"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id").to_string();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got = extract_structured(
            &client.tools_call("host.runs.get", json!({"run_id": run_id})).await.expect("runs.get ok"),
        );
        if got["status"] != json!("queued") && got["status"] != json!("running") {
            assert_eq!(got["status"], json!("done"), "{got}");
            return;
        }
        assert!(Instant::now() < deadline, "run never finalized within 10s");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn setup(name: &str) -> (TestServer, McpClient, String) {
    let server = TestServer::start_with_kinds(chain_and_http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, name).await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    (server, client, ns)
}

#[tokio::test]
async fn force_true_publishes_the_failing_chain_and_says_so() {
    let (_server, client, ns) = setup("Cpath AC6 Tenant").await;
    publish(&client, "src", "echo", json!({"schema": {"type": "object"}})).await;
    publish(&client, "sink", "echo", json!({"schema": {"type": "object"}})).await;
    for n in 1..=3 {
        run_to_done(&client, "src", json!({"items": [1, 2, n], "count": n})).await;
    }
    let spec = json!({"steps": [
        {"tool": "src", "args": {}},
        {"tool": "sink", "args": {"rows": "$.prev.result.rows"}},
    ]});

    let published = extract_structured(
        &client
            .tools_call(
                "host.tool_publish",
                json!({"name": "pipeline", "kind": "chain", "spec": spec, "force": true}),
            )
            .await
            .expect("force: true must publish the failing chain"),
    );

    assert_eq!(published["name"], json!(format!("{ns}.pipeline")), "{published}");
    let gate = published["gates"]
        .as_array()
        .and_then(|g| g.first())
        .unwrap_or_else(|| panic!("a forced publish carries a gates entry: {published}"));
    assert_eq!(gate["verdict"], json!("will_fail"), "{published}");
    assert_eq!(gate["forced"], json!(true), "{published}");

    let list = extract_structured(&client.tools_call("host.tool_list", json!({})).await.expect("list"));
    let names: Vec<&str> = list["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(names.contains(&format!("{ns}.pipeline").as_str()), "the forced chain must exist: {names:?}");
}
