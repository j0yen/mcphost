//! PRD-mcphost-contract-output-schemas R1: the output schema of every
//! canonical `host.*`/`billing.*` tool.
//!
//! One registry, read by every consumer: `handler::host_tools` (so the
//! `outputSchema` in `tools/list` and the contract dump come from the same
//! bytes), `api_contract::dump_contract` and `gendocs`' `Returns` block.
//! The schemas live in `contracts/host-output-schemas.v1.json`, keyed by
//! canonical tool name. They were drafted from the `structuredContent` the
//! integration suite produces (the harness records it under
//! `target/outsch/`) and are kept honest by the `outsch_*` tests, which
//! validate every captured row against them. `required` lists only keys
//! every captured row carried; a tool whose response shape depends on the
//! caller (e.g. `host.whoami` for a tenant, an admin or an implicit
//! connection) declares the union of those shapes.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use serde_json::{Map, Value};

const RAW: &str = include_str!("../contracts/host-output-schemas.v1.json");

type Registry = BTreeMap<String, Arc<Map<String, Value>>>;

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let parsed: BTreeMap<String, Map<String, Value>> =
            serde_json::from_str(RAW).expect("contracts/host-output-schemas.v1.json is valid JSON");
        parsed.into_iter().map(|(k, v)| (k, Arc::new(v))).collect()
    })
}

/// `canonical`'s output schema, `None` when none is registered.
pub fn output_schema_for(canonical: &str) -> Option<Arc<Map<String, Value>>> {
    registry().get(canonical).cloned()
}

/// Every canonical tool name that has a registered output schema, sorted.
pub fn registered_tools() -> Vec<&'static str> {
    registry().keys().map(String::as_str).collect()
}

/// The largest `oneOf`/`anyOf` branch count anywhere in `schema` (0 when it
/// has none) -- the measure PRD AC1's "more than 3 branches" budget counts.
pub fn max_union_branches(schema: &Value) -> usize {
    match schema {
        Value::Object(map) => {
            let here = ["oneOf", "anyOf"]
                .iter()
                .filter_map(|k| map.get(*k).and_then(Value::as_array).map(Vec::len))
                .max()
                .unwrap_or(0);
            map.values().map(max_union_branches).max().unwrap_or(0).max(here)
        }
        Value::Array(items) => items.iter().map(max_union_branches).max().unwrap_or(0),
        _ => 0,
    }
}
