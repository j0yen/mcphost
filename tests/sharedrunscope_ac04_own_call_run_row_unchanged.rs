//! PRD-mcphost-shared-call-run-scope
//! AC4 — Given O calls its own `lookup` synchronously, When O calls
//! `host.runs.list`, Then the run appears under O exactly as before (byte-
//! identical on the existing fixture) -- an own-tenant call never has a
//! `caller`, so `record_call_attributed_with_end_user`'s new
//! `shared_owner_namespace` is `None` and the run row takes the exact same
//! branch it always did (same convention `sharedcall_ac07_own_unqualified_
//! call_unchanged.rs` already pins for the call's own JSON-RPC response;
//! this file pins the `host.runs.list` entry instead).

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

/// Strips the fields that are genuinely non-deterministic per call
/// (ulid run id, wall-clock timestamps/duration) so what remains can be
/// compared byte-for-byte against a fixed expected shape.
fn sanitize(run: &Value) -> Value {
    let mut run = run.clone();
    let obj = run.as_object_mut().expect("run is an object");
    for key in ["run_id", "started_unix", "finished_unix", "duration_ms"] {
        obj.remove(key);
    }
    run
}

#[tokio::test]
async fn own_sync_call_run_row_is_unchanged() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC4 Owner").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "lookup", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("O publishes lookup");

    client
        .tools_call(&format!("{ns}.lookup"), json!({"msg": "hi"}))
        .await
        .expect("O calls its own tool");

    let runs = extract_structured(
        &client
            .tools_call("host.runs.list", json!({}))
            .await
            .expect("O lists its own runs"),
    );
    let list = runs["runs"].as_array().expect("runs array");
    assert_eq!(list.len(), 1, "exactly one run for O: {runs}");
    let sanitized = sanitize(&list[0]);

    assert_eq!(
        sanitized,
        json!({
            "tool": "lookup",
            "trigger": "call",
            "trigger_ref": null,
            "status": "done",
            "progress": null,
            "counters": {},
            "result": null,
            "result_ref": null,
            "purged": false,
            "error_class": null,
            "error": null,
            "deadline_s": null,
            "attempt": 1,
            "manual": false,
            "test": false,
            // PRD-mcphost-runs-end-user-subject (already on main): every
            // run row now reports end_user; an unidentified own-tenant call
            // has none.
            "end_user": null,
        }),
        "own-tenant run row must be byte-identical to the pre-existing shape: {sanitized}"
    );
}
