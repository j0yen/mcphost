//! Shared setup helpers for the "Uptime probes with no server" recipe
//! (PRD-mcphost-uptime-probes, `www/llms.txt`, `examples/uptime-probes/`):
//! the `targets`/`checks` tables, publishing `probe`/`status`, and
//! scheduling/firing `probe` the same way every AC file's own test does.
//!
//! Not a crate module: each `tests/*.rs` file is its own crate root, so
//! this is pulled in per-file with `#[path = "support/uptime_probes.rs"]
//! mod uptime_probes;`, same convention as `tests/support/host.rs`.

use crate::common::{McpClient, extract_structured};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub fn probe_source() -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/uptime-probes/tools/probe.py"),
    )
    .expect("read examples/uptime-probes/tools/probe.py")
}

pub fn status_source() -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/uptime-probes/tools/status.py"),
    )
    .expect("read examples/uptime-probes/tools/status.py")
}

/// Requirement 1: the `targets`/`checks` tables the recipe's own
/// `www/llms.txt` section shows.
pub async fn create_tables(client: &McpClient) {
    client
        .tools_call(
            "host.state.table_create",
            json!({
                "name": "targets",
                "schema": {"url": "text", "added_at": "real"},
                "primary_key": "url",
            }),
        )
        .await
        .expect("targets table_create");
    client
        .tools_call(
            "host.state.table_create",
            json!({
                "name": "checks",
                "schema": {"url": "text", "code": "integer", "ms": "real", "at": "real"},
            }),
        )
        .await
        .expect("checks table_create");
}

pub async fn publish_probe(client: &McpClient) {
    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "probe",
                "kind": "python",
                "spec": {"source": probe_source(), "network": "public"},
            }),
        )
        .await
        .expect("publish probe");
}

pub async fn publish_status(client: &McpClient) {
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "status", "kind": "python", "spec": {"source": status_source()}}),
        )
        .await
        .expect("publish status");
}

/// Sets a schedule trigger on `probe` (the recipe's own `*/5 * * * *`
/// schedule) and returns its `id`.
pub async fn set_probe_schedule(client: &McpClient) -> String {
    let set = extract_structured(
        &client
            .tools_call(
                "host.trigger.set",
                json!({"tool": "probe", "kind": "schedule", "schedule": "*/5 * * * *"}),
            )
            .await
            .expect("trigger.set"),
    );
    set["id"].as_str().expect("trigger id").to_string()
}

/// Fires the trigger once and waits for the resulting run to leave
/// `queued`/`running`, same convention as
/// `tests/sched_ac10_trigger_fire_manual.rs`'s own poll loop.
pub async fn fire_and_wait(client: &McpClient, trigger_id: &str) -> Value {
    let fired = extract_structured(
        &client
            .tools_call("host.trigger.fire", json!({"id": trigger_id}))
            .await
            .expect("trigger.fire"),
    );
    let run_id = fired["run_id"].as_str().expect("run_id").to_string();

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("runs.get"),
        );
        if !matches!(got["status"].as_str(), Some("queued") | Some("running")) {
            return got;
        }
        if Instant::now() >= deadline {
            panic!("probe run never left queued/running within 20s: {got:?}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
