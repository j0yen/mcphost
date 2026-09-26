//! PRD-mcphost-run-result-overflow-to-state
//! AC2 (P0) — Given a tenant with 300 KiB of state quota remaining and a
//! 1.4 MiB async result, When the run finishes, Then `status: "error"`,
//! `error.kind == "state_quota"` with `needed_bytes` and `available_bytes`,
//! no `runs/<id>/part/*` keys exist, and `counters` reported during the run
//! are present.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::{Duration, Instant};

const FREE_STATE_BYTES_MAX: i64 = 5 * 1024 * 1024;
const AVAILABLE_BYTES: i64 = 300 * 1024;

#[tokio::test]
async fn oversized_result_over_quota_fails_state_quota_and_keeps_counters() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "RunOverflow AC2 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .expect("find tenant")
        .expect("tenant exists");

    // Leave this tenant with exactly AVAILABLE_BYTES of its free-plan
    // state_bytes_max quota free, bypassing the RPC's own 1 MiB request-body
    // cap (host.state.set) since the filler itself is ~4.7 MiB.
    let filler_len = (FREE_STATE_BYTES_MAX - AVAILABLE_BYTES - 2).max(0) as usize;
    let filler_value_json = serde_json::to_string(&"x".repeat(filler_len)).expect("serialize filler");
    server
        .state
        .db
        .state_kv_set(tenant.id, "filler".to_string(), String::new(), filler_value_json)
        .await
        .expect("seed filler state");
    let used = server.state.db.state_bytes_used(tenant.id).await.expect("state_bytes_used");
    assert_eq!(
        FREE_STATE_BYTES_MAX - used,
        AVAILABLE_BYTES,
        "test setup must leave exactly {AVAILABLE_BYTES} bytes available"
    );

    let spec = json!({
        "source": "import time\ndef main(args):\n    time.sleep(2)\n    return {\"blob\": \"x\" * args.get(\"n\", 0)}\n",
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "quotajob", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");
    let qualified = format!("{ns}.quotajob");
    let _ = poll_until_ready(&client, &qualified, json!({"n": 1}), Duration::from_secs(15))
        .await
        .expect("warm-up call ok");

    let n = 1_468_006usize; // ~1.4 MiB, well over the 300 KiB left.
    let enqueue = extract_structured(
        &client
            .tools_call(
                "host.tool_call",
                json!({"name": "quotajob", "args": {"n": n}, "async": true}),
            )
            .await
            .expect("enqueue ok"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id").to_string();

    // Wait for the job to actually start running, then report a counter
    // mid-run (simulating "the tool reports" -- host.progress is callable
    // by id from outside the running tool's own process too).
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("runs.get ok"),
        );
        if got["status"] == json!("running") {
            break;
        }
        assert!(Instant::now() < deadline, "run never reached running");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    extract_structured(
        &client
            .tools_call(
                "host.progress",
                json!({"run_id": run_id, "counters": {"items_processed": 777}}),
            )
            .await
            .expect("progress ok"),
    );

    let done = extract_structured(
        &client
            .tools_call("host.runs.wait", json!({"run_id": run_id, "timeout_s": 20}))
            .await
            .expect("wait ok"),
    );
    assert_eq!(done["status"], json!("error"), "run: {done}");
    assert_eq!(done["error"]["kind"], json!("state_quota"), "run: {done}");
    let needed_bytes = done["error"]["data"]["needed_bytes"]
        .as_i64()
        .expect("needed_bytes must be present and numeric");
    let available_bytes = done["error"]["data"]["available_bytes"]
        .as_i64()
        .expect("available_bytes must be present and numeric");
    assert!(needed_bytes > available_bytes, "run: {done}");
    assert_eq!(available_bytes, AVAILABLE_BYTES, "run: {done}");
    assert_eq!(
        done["counters"]["items_processed"],
        json!(777),
        "counters reported during the run must survive the state_quota finalize: {done}"
    );

    let part0 = server
        .state
        .db
        .state_kv_get(tenant.id, format!("runs/{run_id}/part/0"), String::new())
        .await
        .expect("state_kv_get part0");
    assert!(part0.is_none(), "no runs/<id>/part/* key must exist after a state_quota failure");
}
