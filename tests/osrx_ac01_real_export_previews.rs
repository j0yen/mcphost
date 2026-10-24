//! contract: reads registry handler::host_tool_descriptors
//! PRD-mcphost-import-dry-run-live-real-export
//! AC1 (P0) -- Given a source tenant whose `host.export` of one state key reached
//! `done` and a fresh destination tenant, When the destination calls
//! `host.import` with `dry_run: true` and that export's `run_id`, then with
//! `run_id` "x", Then the first is a non-error preview with >= 1 `entries`
//! pattern that the served `outputSchema` admits through the own-dry-run branch,
//! and the second is the not-found error.

use crate::common::TestServer;
use crate::osreach;
use crate::timp;
use serde_json::json;

#[tokio::test]
async fn osrx_ac01_a_real_run_id_previews_and_the_placeholder_is_not_found() {
    let server = TestServer::start().await;
    let source = timp::new_tenant(&server, "Osrx AC1 Source").await;
    source.call("host.state.set", json!({"key": "k1", "value": {"n": 1}})).await;
    let (run_id, _) = source.export(&server).await;
    let dest = timp::new_tenant(&server, "Osrx AC1 Dest").await;

    let listed = dest.client.tools_list().await.expect("tools/list");
    let import = listed["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|t| t["name"].as_str().is_some_and(|n| mcphost::import::IMPORT_TOOL_NAME == osreach::canonical_of(n)))
        .expect("host.import is served")
        .clone();
    let name = import["name"].as_str().expect("name").to_string();
    let schema = import["outputSchema"].clone();

    let preview = dest.call(&name, json!({"run_id": run_id, "dry_run": true})).await;
    assert_eq!(preview["dry_run"], json!(true), "{preview}");
    let entries = preview["entries"].as_object().expect("entries is a pattern -> count map");
    assert!(!entries.is_empty(), "preview lists at least one entry pattern: {preview}");
    osreach::check(&name, osreach::STATE_DRY_RUN, &schema, &preview).expect("served schema admits the preview");
    assert_eq!(osreach::variant_of(osreach::STATE_DRY_RUN, &preview), Some(osreach::VARIANT_OWN_DRY_RUN));

    // The sweep's placeholder referent is schema-valid but not state-valid.
    let err = dest
        .client
        .tools_call(&name, json!({"run_id": "x", "dry_run": true}))
        .await
        .expect_err("a placeholder run_id is the not-found error");
    assert!(err.message.contains("no completed host.export matches"), "{} {}", err.code, err.message);
}
