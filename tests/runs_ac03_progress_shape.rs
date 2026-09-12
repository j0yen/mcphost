//! AC3 (P0) — Given a running job reporting `progress(50, "half")`, When
//! `host.runs.get` is read, Then `progress` is `{pct: 50, msg: "half"}`.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn progress_reads_back_as_pct_and_msg() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC3 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import time, mcphost\n\
def main(args):\n    \
    mcphost.progress(50, \"half\")\n    \
    time.sleep(3)\n    \
    return {\"ok\": True}\n",
        "args_schema": {"type": "object"},
        "timeout_s": 20,
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "reports_progress", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");
    let _ = poll_until_ready(
        &client,
        &format!("{ns}.reports_progress"),
        json!({}),
        Duration::from_secs(15),
    )
    .await;

    let enqueue = extract_structured(
        &client
            .tools_call(
                "host.tool_call",
                json!({"name": "reports_progress", "args": {}, "async": true}),
            )
            .await
            .expect("enqueue ok"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id").to_string();

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("runs.get ok"),
        );
        if got["progress"] == json!({"pct": 50, "msg": "half"}) {
            return;
        }
        assert!(Instant::now() < deadline, "progress never observed within 15s: {got}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
