//! PRD-mcphost-dry-run-on-write-verbs
//! AC1 -- Given a tenant with table `orders` (100 rows), When `host.table.drop
//! {name: "orders"}` runs inside `dryrun::with_dry_run` + `finish`, Then
//! `host.table.list` still shows `orders` with 100 rows and `writes()` holds
//! one `{store: "table", op: "drop", table: "orders"}` entry. Evidence for the
//! PRD's falsifier: DDL does NOT escape the savepoint (SQLite `DROP TABLE` is
//! transactional), so R3 observes the diff instead of computing it.

use crate::common;
use mcphost::dryrun;
use mcphost::tables;
use serde_json::json;

#[tokio::test]
async fn table_drop_inside_the_dry_run_scope_rolls_back_and_records_the_write() {
    let (state, _dir) = common::bare_app_state().await;
    let tenant = state
        .db
        .create_tenant("Dryv AC1".to_string(), "dryv-ac1".to_string(), "key-hash-dryv-ac1".to_string(), None)
        .await
        .expect("create_tenant");

    tables::table_create(&state, &tenant, &json!({"name": "orders", "columns": {"sku": "text"}}))
        .await
        .expect("create orders");
    let rows: Vec<_> = (0..100).map(|i| json!({"sku": format!("sku-{i}")})).collect();
    tables::table_append(&state, &tenant, &json!({"table": "orders", "rows": rows}))
        .await
        .expect("append 100 rows");

    let ctx = dryrun::DryRunCtx::new(state.db.path().to_path_buf(), state.db.cfg());
    let outcome = dryrun::with_dry_run(
        ctx.clone(),
        tables::table_drop(&state, &tenant, &json!({"name": "orders"})),
    )
    .await;
    dryrun::finish(ctx.clone()).await;
    assert_eq!(outcome.expect("drop ok inside the scope")["dropped"], json!(true));

    assert_eq!(
        ctx.writes(),
        vec![json!({"store": "table", "op": "drop", "table": "orders"})],
        "one drop write, exactly"
    );

    let listed = tables::table_list(&state, &tenant, &json!({})).await.expect("table list");
    let orders = listed["tables"]
        .as_array()
        .expect("tables array")
        .iter()
        .find(|t| t["name"] == json!("orders"))
        .unwrap_or_else(|| panic!("DDL escapes savepoint: orders is gone: {listed}"));
    assert_eq!(orders["rows"], json!(100), "{orders}");
}
