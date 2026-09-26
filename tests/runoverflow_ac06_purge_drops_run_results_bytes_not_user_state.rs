//! PRD-mcphost-run-result-overflow-to-state
//! AC6 (P0) — Given a run with 6 parts is purged by retention, When
//! `host.usage` (the existing usage surface `run_results_bytes` lives on)
//! is read before and after, Then `run_results_bytes` drops by the parts'
//! size and user state is unchanged.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn purge_drops_run_results_bytes_and_leaves_user_state_alone() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "RunOverflow AC6 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // This tenant's own state, unrelated to any run -- must survive the
    // purge byte-for-byte.
    client
        .tools_call("host.state.set", json!({"key": "mykey", "value": "hello"}))
        .await
        .expect("seed user state ok");

    let spec = json!({
        "source": "def main(args):\n    return {\"blob\": \"x\" * args.get(\"n\", 0)}\n",
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "bigjob", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");
    let qualified = format!("{ns}.bigjob");
    let _ = poll_until_ready(&client, &qualified, json!({"n": 1}), Duration::from_secs(15))
        .await
        .expect("warm-up call ok");

    // 1.4 MiB (the arg is just a small integer over the wire -- the sandbox
    // itself builds the big string) -> 6 parts, same math as AC1.
    let enqueue = extract_structured(
        &client
            .tools_call(
                "host.tool_call",
                json!({"name": "bigjob", "args": {"n": 1_468_006}, "async": true}),
            )
            .await
            .expect("enqueue ok"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id").to_string();

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut done = None;
    while Instant::now() < deadline {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("runs.get ok"),
        );
        if got["status"] == json!("done") {
            done = Some(got);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let done = done.expect("echo job must finish well within 5s");
    assert_eq!(done["result_ref"]["parts"], json!(6), "run: {done}");

    let before = extract_structured(
        &client.tools_call("host.usage", json!({})).await.expect("usage before ok"),
    );
    let run_results_before = before["run_results_bytes"].as_i64().expect("run_results_bytes");
    let state_before = before["state_bytes"].as_i64().expect("state_bytes");
    assert!(run_results_before > 0, "before: {before}");
    let user_before = state_before - run_results_before;

    let now = mcphost::state::now_unix();
    let purge = extract_structured(
        &client
            .tools_call("host.runs.purge", json!({"before_unix": now}))
            .await
            .expect("purge ok"),
    );
    assert_eq!(purge["purged"], json!(1), "one run must be purged: {purge}");

    let after = extract_structured(
        &client.tools_call("host.usage", json!({})).await.expect("usage after ok"),
    );
    let run_results_after = after["run_results_bytes"].as_i64().expect("run_results_bytes");
    let state_after = after["state_bytes"].as_i64().expect("state_bytes");
    assert_eq!(run_results_after, 0, "after: {after}");
    assert_eq!(
        state_before - state_after,
        run_results_before,
        "state_bytes must drop by exactly the purged parts' size: before={before} after={after}"
    );
    let user_after = state_after - run_results_after;
    assert_eq!(user_after, user_before, "user state bytes must be unchanged by the purge");

    let mykey = extract_structured(
        &client
            .tools_call("host.state.get", json!({"key": "mykey"}))
            .await
            .expect("state.get after purge ok"),
    );
    assert_eq!(mykey["value"], json!("hello"), "user state's own value must survive: {mykey}");
}
