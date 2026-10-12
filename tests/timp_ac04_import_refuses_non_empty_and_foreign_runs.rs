//! PRD-mcphost-export-tables-and-import-restore
//! AC4 -- Given tenant B holds one tool, When `host.import` runs, Then
//! `tenant_not_empty` lists the tool and nothing is written; Given another
//! tenant's run id that is not a completed export, Then `not_found`. (The tool is named `xx`: tool names
//! are `^[a-z][a-z0-9_]{1,40}$`, so a one-letter `x` cannot be published.)

use crate::common;
use crate::timp;

use common::TestServer;
use serde_json::json;

#[tokio::test]
async fn non_empty_tenant_is_refused_with_its_contents_and_nothing_is_written() {
    let server = TestServer::start().await;
    let a = timp::new_tenant(&server, "Timp AC4 A").await;
    a.seed_full().await;
    let (run_id, _) = a.export(&server).await;

    let b = timp::new_tenant(&server, "Timp AC4 B").await;
    b.call("host.tool_publish", json!({"name": "xx", "kind": "echo", "spec": {"schema": {"type": "object"}}}))
        .await;
    let err = b
        .client
        .tools_call("host.import", json!({"run_id": run_id}))
        .await
        .expect_err("a non-empty tenant must be refused");
    assert_eq!(err.error_code.as_deref(), Some("tenant_not_empty"), "{err:?}");
    assert_eq!(err.data["tools"], json!(["xx"]), "{:?}", err.data);
    assert_eq!(err.data["tables"], json!([]), "{:?}", err.data);
    assert_eq!(err.data["state_keys"], json!([]), "{:?}", err.data);
    assert_eq!(err.data["docs"], json!([]), "{:?}", err.data);

    // Nothing was written.
    assert_eq!(b.call("host.table.list", json!({})).await["tables"], json!([]));
    assert_eq!(b.call("host.docs.list", json!({})).await["documents"], json!([]));
    assert_eq!(b.call("host.state.list", json!({})).await["keys"].as_array().map_or(0, Vec::len), 0);
}

#[tokio::test]
async fn another_tenants_run_id_that_is_not_a_completed_export_is_not_found() {
    let server = TestServer::start().await;
    let a = timp::new_tenant(&server, "Timp AC4 Owner").await;
    a.seed_full().await;
    let (export_id, _) = a.export(&server).await;
    // B imports A's export; the import's own run id belongs to B.
    let b = timp::new_tenant(&server, "Timp AC4 Importer").await;
    let done = b.import_run(&export_id).await;
    assert_eq!(done["status"], json!("done"), "{done}");
    let b_import_run = done["id"].as_str().or(done["run_id"].as_str()).expect("run id").to_string();

    let c = timp::new_tenant(&server, "Timp AC4 Stranger").await;
    for foreign in [b_import_run, "01ZZZZZZZZZZZZZZZZZZZZZZZZ".to_string()] {
        let err = c
            .client
            .tools_call("host.import", json!({"run_id": foreign}))
            .await
            .expect_err("a run that is not a completed host.export must be refused");
        assert_eq!(err.error_code.as_deref(), Some("not_found"), "{foreign}: {err:?}");
    }
}
