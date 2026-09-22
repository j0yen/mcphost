//! PRD-mcphost-database-in-a-minute
//! AC9 — Given `csv_import`, When given the fixture text, Then the table
//! holds 1,000 rows without the caller batching.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

const FIXTURE_CSV: &str = include_str!("../examples/database-in-a-minute/fixture.csv");

#[tokio::test]
async fn csv_import_loads_all_1000_rows_from_one_call() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC9 Owner").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.state.table_create",
            json!({
                "name": "expenses",
                "schema": {"id": "integer", "category": "text", "amount": "real", "day": "integer"},
                "primary_key": "id",
            }),
        )
        .await
        .expect("table create");

    let csv_import_source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples/database-in-a-minute/tools/csv_import.py"),
    )
    .expect("read csv_import.py");
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "csv_import", "kind": "python", "spec": {"source": csv_import_source}}),
        )
        .await
        .expect("publish csv_import");

    let qualified = format!("{ns}.csv_import");

    // Exactly one call, the whole fixture text -- the caller does no
    // batching of its own; csv_import.py loops internally
    // (BATCH_SIZE = state_ops_per_call_max) to stay under the host's
    // per-call row cap.
    let imported = poll_until_ready(
        &client,
        &qualified,
        json!({"table": "expenses", "csv_text": FIXTURE_CSV}),
        Duration::from_secs(20),
    )
    .await
    .unwrap_or_else(|e| panic!("csv_import must succeed: {} {}", e.code, e.message));
    let imported_struct = extract_structured(&imported);
    assert_eq!(
        imported_struct["inserted"],
        json!(1000),
        "csv_import must report all 1000 rows inserted"
    );

    let queried = client
        .tools_call("host.state.query", json!({"table": "expenses"}))
        .await
        .expect("host.state.query");
    let rows = extract_structured(&queried)["rows"]
        .as_array()
        .expect("rows array")
        .len();
    assert_eq!(rows, 1000, "the table must hold exactly 1000 rows after the single csv_import call");
}
