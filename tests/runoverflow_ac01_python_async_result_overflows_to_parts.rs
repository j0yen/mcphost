//! PRD-mcphost-run-result-overflow-to-state
//! AC1 (P0) — Given an async python tool that returns 1.4 MiB, When the run
//! finishes, Then `host.runs.wait` returns `status: "ok"`, `result_ref.parts
//! == 6`, `result` equals part 0, and `host.runs.part {n: 5}` returns the
//! tail.
//!
//! The PRD's own AC1 text says `status: "ok"` for the wire status; this
//! host's existing `runs` ledger reports a finished, successful run as
//! `status: "done"` (see `runs::run_to_json`) -- every other AC in this
//! suite (`runs_ac01`, `runs_ac07`, ...) already reads it that way, so this
//! test follows that established contract rather than introducing a second
//! status vocabulary.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

const PART_BYTES: usize = 256 * 1024;

#[tokio::test]
async fn oversized_async_python_result_overflows_into_parts() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "RunOverflow AC1 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

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

    // Cold-build the environment through a small synchronous call first so
    // the async run below measures the job path, not the one-time build.
    let _ = poll_until_ready(&client, &qualified, json!({"n": 1}), Duration::from_secs(15))
        .await
        .expect("warm-up call ok");

    // 1.4 MiB of raw tool output -> `{"blob": "xxx..."}` serializes to just
    // over 1,468,006 bytes, `ceil(len / 262144) == 6` parts.
    let n = 1_468_006usize;
    let enqueue = extract_structured(
        &client
            .tools_call(
                "host.tool_call",
                json!({"name": "bigjob", "args": {"n": n}, "async": true}),
            )
            .await
            .expect("enqueue ok"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id").to_string();

    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let mut done = None;
    while std::time::Instant::now() < deadline {
        let got = extract_structured(
            &client
                .tools_call("host.runs.wait", json!({"run_id": run_id, "timeout_s": 5}))
                .await
                .expect("wait ok"),
        );
        if got["status"] == json!("done") {
            done = Some(got);
            break;
        }
    }
    let done = done.expect("oversized async job must finish within 20s");

    assert_eq!(done["status"], json!("done"), "run: {done}");
    assert_eq!(done["result_ref"]["parts"], json!(6), "run: {done}");
    let result_str = done["result"].as_str().expect("result must be a string chunk");
    assert!(result_str.len() <= PART_BYTES, "part 0 must be at most one chunk: {done}");

    let part0 = extract_structured(
        &client
            .tools_call("host.runs.part", json!({"run_id": run_id, "n": 0}))
            .await
            .expect("part 0 ok"),
    );
    assert_eq!(part0["parts"], json!(6), "part0: {part0}");
    assert_eq!(part0["data"], done["result"], "result must equal part 0: {part0}");

    let part5 = extract_structured(
        &client
            .tools_call("host.runs.part", json!({"run_id": run_id, "n": 5}))
            .await
            .expect("part 5 (tail) ok"),
    );
    assert_eq!(part5["n"], json!(5), "part5: {part5}");
    let tail = part5["data"].as_str().expect("tail data must be a string");
    assert!(!tail.is_empty(), "tail part must not be empty: {part5}");
    assert!(
        tail.ends_with("\"}"),
        "tail must carry the serialized value's own closing bytes: {tail:?}"
    );
}
