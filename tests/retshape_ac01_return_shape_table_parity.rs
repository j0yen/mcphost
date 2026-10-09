//! PRD-mcphost-sandbox-return-shape-contract
//! AC1 (P0) -- Given the return-shape table, When the parity test calls each
//! envelope-returning bridge function on a fixture tenant, Then the real
//! top-level keys equal the table's keys for every function, and
//! `mcphost.state.query`/`mcphost.table.query` are both present.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::kinds::python::{
    BRIDGE_MODULES, BRIDGE_RETURNS, BridgeModule, BridgeReturn, ReturnShape, bridge_functions_without_return_row,
};
use mcphost::lineage::{self, NodeKind};
use mcphost::sandbox;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

/// A tool that calls every bridge function once on a fixture tenant and
/// reports, per function, the sorted top-level keys of what really came
/// back (`null` when the result is not an object). Each call is isolated so
/// one refusal is reported rather than hiding the rest.
pub const PROBE_SOURCE: &str = r#"import mcphost

def _keys(v):
    return sorted(dict.keys(v)) if isinstance(v, dict) else None

def main(args):
    out = {}
    state, table = mcphost.state, mcphost.table
    steps = [
        ("mcphost.state.table_create", lambda: state.table_create("rs_t", {"n": "integer"})),
        ("mcphost.state.insert", lambda: state.insert("rs_t", [{"n": 1}])),
        ("mcphost.state.query", lambda: state.query(table="rs_t")),
        ("mcphost.state.delete_rows", lambda: state.delete_rows("rs_t")),
        ("mcphost.state.set", lambda: state.set("rs_k", 1)),
        ("mcphost.state.list", lambda: state.list()),
        ("mcphost.state.delete", lambda: state.delete("rs_k")),
        ("mcphost.state.table_drop", lambda: state.table_drop("rs_t")),
        ("mcphost.table.create", lambda: table.create("orders", {"n": "integer"})),
        ("setup.scratch_table", lambda: table.create("rs_scratch", {"n": "integer"})),
        ("mcphost.table.append", lambda: table.append("orders", [{"n": 1}])),
        ("mcphost.table.query", lambda: table.query("SELECT n FROM orders")),
        ("mcphost.table.query(handle=True)", lambda: table.query("SELECT n FROM orders", handle=True)),
        ("mcphost.table.list", lambda: table.list()),
        ("mcphost.table.schema", lambda: table.schema("orders")),
        ("mcphost.table.chart", lambda: table.chart("SELECT n FROM orders")),
        ("mcphost.docs.search", lambda: mcphost.docs.search("anything")),
        ("mcphost.lineage.trace", lambda: mcphost.lineage.trace("table:orders")),
        ("mcphost.lineage.blast_radius", lambda: mcphost.lineage.blast_radius("table:orders", "drop")),
        ("mcphost.drift.reviews", lambda: mcphost.drift.reviews()),
        ("mcphost.channel.post", lambda: mcphost.channel.post(args["cid"], {"n": 1})),
        ("mcphost.channel.read", lambda: mcphost.channel.read(args["cid"])),
        ("mcphost.msg.send", lambda: mcphost.msg.send(args["to"], {"hello": 1})),
        ("mcphost.msg.inbox", lambda: mcphost.msg.inbox()),
        ("mcphost.table.drop", lambda: table.drop("rs_scratch")),
    ]
    for name, fn in steps:
        try:
            out[name] = {"keys": _keys(fn())}
        except Exception as e:
            out[name] = {"error": type(e).__name__ + ": " + str(e)}
    return out
"#;

/// Runs [`PROBE_SOURCE`] on a fresh fixture tenant; `None` when the host has
/// no unprivileged user namespaces (same skip convention as every sandbox
/// test). Returns `{function: {"keys": [...]} | {"error": ...}}`.
pub async fn probe_real_returns() -> Option<BTreeMap<String, Value>> {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return None;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Retshape Probe Tenant").await;
    let (ns_b, _key_b) = signup(&server.base_url, "Retshape Probe Peer").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .expect("db")
        .expect("tenant exists");
    lineage::register_edge(
        &server.state,
        tenant.id,
        (NodeKind::Table, "orders", "orders"),
        (NodeKind::Tool, "orders_reader", "orders_reader"),
        "declared_reads",
    )
    .await
    .expect("register edge");

    client.tools_call("host.group.create", json!({"name": "rs"})).await.expect("group.create");
    let opened = extract_structured(
        &client.tools_call("host.channel.open", json!({"group": "rs"})).await.expect("channel.open"),
    );
    let cid = opened["channel_id"].as_str().expect("channel_id").to_string();

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "probe", "kind": "python", "spec": {"source": PROBE_SOURCE}}),
        )
        .await
        .expect("publish probe");
    let result = extract_structured(
        &poll_until_ready(&client, &format!("{ns}.probe"), json!({"cid": cid, "to": ns_b}), Duration::from_secs(20))
            .await
            .unwrap_or_else(|e| panic!("probe call must succeed: {} {}", e.code, e.message)),
    );
    Some(result.as_object().expect("probe returns an object").clone().into_iter().collect())
}

fn envelope_keys(r: &BridgeReturn) -> Option<BTreeSet<String>> {
    match r.shape {
        ReturnShape::Envelope(keys) => Some(keys.iter().map(|(k, _)| k.to_string()).collect()),
        ReturnShape::Plain(_) => None,
    }
}

#[tokio::test]
async fn real_top_level_keys_equal_the_tables_for_every_envelope_function() {
    let Some(real) = probe_real_returns().await else { return };

    let mut checked = BTreeSet::new();
    for row in BRIDGE_RETURNS {
        let Some(want) = envelope_keys(row) else { continue };
        let Some(got) = real.get(row.function) else {
            panic!("probe did not exercise {} -- extend PROBE_SOURCE: {real:#?}", row.function);
        };
        let keys: BTreeSet<String> = got["keys"]
            .as_array()
            .unwrap_or_else(|| panic!("{} did not return an object: {got}", row.function))
            .iter()
            .map(|k| k.as_str().unwrap().to_string())
            .collect();
        assert_eq!(keys, want, "{}: real keys differ from the return-shape table", row.function);
        checked.insert(row.function);
    }
    assert!(checked.contains("mcphost.state.query"), "state.query must be in the table: {checked:?}");
    assert!(checked.contains("mcphost.table.query"), "table.query must be in the table: {checked:?}");
}

#[test]
fn every_bridge_function_has_a_table_row_and_a_fixture_module_without_one_is_caught() {
    assert_eq!(
        bridge_functions_without_return_row(BRIDGE_MODULES, BRIDGE_RETURNS),
        Vec::<String>::new(),
        "every BRIDGE_MODULES function needs a BRIDGE_RETURNS row"
    );

    let fixture = BridgeModule {
        name: "fixture_test_module",
        purpose: "a module with no return-shape row",
        signatures: &["mcphost.fixture_test_module.ping() -- always returns true"],
    };
    let mut extended: Vec<BridgeModule> = BRIDGE_MODULES.to_vec();
    extended.push(fixture);
    assert_eq!(
        bridge_functions_without_return_row(&extended, BRIDGE_RETURNS),
        vec!["mcphost.fixture_test_module.ping".to_string()]
    );
}
