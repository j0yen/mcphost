//! PRD-mcphost-team-memory
//! AC5 — Given `recall` with `limit` 500, When called, Then at most 50
//! rows are returned.
//!
//! Self-contained (no `proof.sh`, no groups): a single tenant seeds 55
//! matching rows directly via `mcphost.table.append` (fast -- one sandbox
//! call, not 55 `remember` round trips) and calls the real
//! `examples/team-memory/tools/recall.py` source with `limit: 500`.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

const SEED_ROWS: usize = 55;

#[tokio::test]
async fn recall_caps_at_50_rows_even_when_limit_500_is_requested() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({
                "name": "memory",
                "columns": {"key": "text", "text": "text", "tags": "json", "writer": "text", "at": "real"},
            }),
        )
        .await
        .expect("table create");

    let seed_source = format!(
        "import mcphost\ndef main(args):\n    rows = [{{\"key\": f\"probe-{{i}}\", \"text\": \"cap probe\", \"tags\": [\"cap-test\"], \"writer\": \"seed\", \"at\": float(i)}} for i in range({SEED_ROWS})]\n    mcphost.table.append(table=\"memory\", rows=rows)\n    return {{\"seeded\": len(rows)}}\n"
    );
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "seed", "kind": "python", "spec": {"source": seed_source}}),
        )
        .await
        .expect("publish seed");
    let seeded = poll_until_ready(&client, &format!("{ns}.seed"), json!({}), Duration::from_secs(10))
        .await
        .unwrap_or_else(|e| panic!("seed call must succeed: {} {}", e.code, e.message));
    assert_eq!(extract_structured(&seeded)["seeded"], json!(SEED_ROWS));

    let recall_source =
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("examples/team-memory/tools/recall.py"),
        )
        .expect("read examples/team-memory/tools/recall.py");
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "recall", "kind": "python", "spec": {"source": recall_source}}),
        )
        .await
        .expect("publish recall");

    let called = poll_until_ready(
        &client,
        &format!("{ns}.recall"),
        json!({"query": "cap-test", "limit": 500}),
        Duration::from_secs(10),
    )
    .await
    .unwrap_or_else(|e| panic!("recall call must succeed: {} {}", e.code, e.message));
    let rows = extract_structured(&called)["rows"]
        .as_array()
        .expect("rows array")
        .clone();

    assert!(
        rows.len() <= 50,
        "recall must cap at 50 rows even when limit=500 is requested, got {}",
        rows.len()
    );
    assert_eq!(
        rows.len(),
        50,
        "expected exactly 50 of the {SEED_ROWS} seeded matching rows, got {}",
        rows.len()
    );
}
