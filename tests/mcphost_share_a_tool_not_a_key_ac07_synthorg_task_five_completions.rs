//! PRD-mcphost-share-a-tool-not-a-key
//! AC7 — Given the synthorg mcphost corpus, When the new task runs 5
//! times, Then 5 completions are recorded with wall time.
//!
//! The synthorg mcphost corpus (`examples/share-a-tool/synthorg-task.yaml`,
//! segment `integration_specialist`) lives in a separate repository
//! (RedBaron's `corpora/mcphost/consumer-tasks.yaml`) this build is not
//! authorized to touch or execute -- see this worktree's own scope
//! (`build_into: mcphost` only). `examples/share-a-tool/proof.sh` is the
//! runnable embodiment of that task's recipe: the same six calls, the
//! same success condition (a shared tool call reaches the upstream
//! without the secret ever reaching the caller). This test runs it five
//! times end to end against five fresh in-process servers and records a
//! completion plus wall time for each, the same shape a synthorg panel
//! run records per session (RedBaron's own `consume.py` records one
//! completion + `latency_ms`/wall time per session, scored against the
//! task's `gold.call_check`).

use crate::common;
use common::{TestServer, http_kind_registry};
use tokio::process::Command;

struct Completion {
    wall_time_ms: u64,
}

async fn run_task_once() -> Option<Completion> {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let mcphost_url = format!("{}/mcp", server.base_url);
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/share-a-tool/proof.sh");
    let output = Command::new("bash")
        .arg(&script)
        .env("MCPHOST_URL", &mcphost_url)
        .env_remove("UPSTREAM_URL")
        .output()
        .await
        .expect("run examples/share-a-tool/proof.sh");
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let wall_time_ms = stdout
        .lines()
        .find_map(|l| l.strip_prefix("WALL_TIME_MS="))
        .and_then(|v| v.parse::<u64>().ok())?;
    Some(Completion { wall_time_ms })
}

#[tokio::test]
async fn synthorg_task_runs_five_times_with_five_completions_and_wall_time() {
    let mut completions = Vec::new();
    for _ in 0..5 {
        if let Some(completion) = run_task_once().await {
            completions.push(completion);
        }
    }

    assert_eq!(
        completions.len(),
        5,
        "expected 5 completions from 5 runs of the share-a-tool-not-a-key task"
    );
    for completion in &completions {
        assert!(
            completion.wall_time_ms > 0,
            "each completion must record a wall time"
        );
    }
}
