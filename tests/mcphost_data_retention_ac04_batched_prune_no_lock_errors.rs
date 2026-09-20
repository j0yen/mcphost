//! PRD-mcphost-data-retention
//! AC4 (P0) — Given 20,000 expired rows, When the prune runs, Then it
//! deletes in batches and no concurrent `host.tool_call` fails with
//! `database is locked`.
//!
//! The prune's batched deletes run on their own dedicated SQLite
//! connection (`retention::prune_sync`), separate from the app's shared
//! one `host.tool_call` writes through -- both now set `busy_timeout`
//! (see `Db::open`/`retention::PRUNE_BUSY_TIMEOUT`), so the two
//! connections genuinely contend for SQLite's single write lock while
//! this test's concurrent callers race the prune, instead of only ever
//! serializing through one in-process mutex.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::common;
use common::{McpClient, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn twenty_thousand_row_prune_batches_without_lock_errors() {
    let server = common::TestServer::start().await;
    let (tenant_ns, key) = signup(&server.base_url, "AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let publish = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "pingpong", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish echo tool");
    let qualified = extract_structured(&publish)["name"]
        .as_str()
        .expect("name field")
        .to_string();

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(tenant_ns)
        .await
        .expect("query tenant")
        .expect("tenant exists");

    server
        .state
        .db
        .set_retention_days_for_test("calls".to_string(), 90)
        .await
        .expect("set calls retention window");

    let now = mcphost::state::now_unix();
    let ninety_one_days_ago = now - 91 * 86_400;
    server
        .state
        .db
        .insert_calls_rows_bulk_for_test(tenant.id, 20_000, ninety_one_days_ago)
        .await
        .expect("bulk-insert 20,000 expired calls rows");

    let stop = Arc::new(AtomicBool::new(false));
    let mut handles = Vec::new();
    for _ in 0..8 {
        let base_url = server.base_url.clone();
        let key = key.clone();
        let qualified = qualified.clone();
        let stop = stop.clone();
        handles.push(tokio::spawn(async move {
            let racer = McpClient::with_bearer(&base_url, &key);
            let mut errors: Vec<String> = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                if let Err(e) = racer.tools_call(&qualified, json!({})).await {
                    errors.push(format!(
                        "{}: {}",
                        e.error_code.clone().unwrap_or_default(),
                        e.message
                    ));
                }
            }
            errors
        }));
    }

    let report = server.state.db.prune_once().await.expect("prune_once");
    stop.store(true, Ordering::Relaxed);

    let mut all_errors = Vec::new();
    for h in handles {
        all_errors.extend(h.await.expect("racer task join"));
    }

    assert!(
        all_errors
            .iter()
            .all(|e| !e.to_lowercase().contains("locked")),
        "concurrent host.tool_call must never fail with 'database is locked' during a prune: {all_errors:?}"
    );
    assert_eq!(
        report.deleted.get("calls").copied(),
        Some(20_000),
        "all 20,000 expired rows must be deleted across however many batches it took, deleted={:?}",
        report.deleted
    );
}
