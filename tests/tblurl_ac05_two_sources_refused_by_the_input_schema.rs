//! PRD-mcphost-table-import-from-url
//! AC5 (P0) — Given a call supplying both `rows` and `source_url`, When
//! host-args validation runs, Then it is refused by the inputSchema `oneOf`
//! before the handler runs, with the refusal naming both fields.

use crate::common;

use common::sypul::{Fixture, orders_csv};
use common::{McpClient, TestServer, signup};
use mcphost::host_args::handler_runs;
use serde_json::{Value, json};

async fn refused(client: &McpClient, ns: &str, tool: &str, args: Value, fields: &[&str]) {
    let before = handler_runs(ns, tool);
    let err = client.tools_call(tool, args).await.expect_err(tool);
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    assert_eq!(err.data["constraint"], json!("oneOf"), "refused by the schema's oneOf: {:?}", err.data);
    for field in fields {
        assert!(err.message.contains(field), "the refusal names {field}: {}", err.message);
    }
    assert_eq!(handler_runs(ns, tool), before, "{tool}: the handler never ran");
}

#[tokio::test]
async fn tblurl_ac05_rows_with_source_url_is_refused_naming_both_fields() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "TBLURL AC5").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client.tools_call("host.table.create", json!({"name": "orders", "columns": {"id": "integer"}})).await.expect("create");
    let fixture = Fixture::start(&orders_csv(5), false).await;

    refused(
        &client,
        &ns,
        "host.table.append",
        json!({"table": "orders", "rows": [{"id": 1}], "source_url": fixture.csv_url()}),
        &["rows", "source_url"],
    )
    .await;
    refused(
        &client,
        &ns,
        "host.table.append",
        json!({"table": "orders", "csv_text": "id\n1\n", "source_url": fixture.csv_url()}),
        &["csv_text", "source_url"],
    )
    .await;
    refused(
        &client,
        &ns,
        "host.table.create",
        json!({"name": "both", "columns": {"id": "integer"}, "source_url": fixture.csv_url()}),
        &["columns", "source_url"],
    )
    .await;
    assert_eq!(fixture.hits(), 0, "nothing was fetched");
}

#[tokio::test]
async fn tblurl_ac05_none_of_the_sources_is_refused_and_each_one_alone_is_accepted() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "TBLURL AC5 alone").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client.tools_call("host.table.create", json!({"name": "orders", "columns": {"id": "integer"}})).await.expect("create");

    let before = handler_runs(&ns, "host.table.append");
    let err = client.tools_call("host.table.append", json!({"table": "orders"})).await.expect_err("no source");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    assert_eq!(handler_runs(&ns, "host.table.append"), before, "the handler never ran");

    client.tools_call("host.table.append", json!({"table": "orders", "rows": [{"id": 1}]})).await.expect("rows alone");
    client.tools_call("host.table.append", json!({"table": "orders", "csv_text": "id\n2\n"})).await.expect("csv_text alone");
}
