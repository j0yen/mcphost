//! PRD-mcphost-server-capabilities-from-registry: the one registry of MCP
//! primitives this host speaks. `initialize.capabilities`, `GET
//! /capabilities.json`, the `## Capabilities` section of `www/llms.txt` and
//! `docs/protocol/capabilities.md`, and the method -> capability map the
//! drift test walks are all generated from [`CAPABILITIES`]; a sibling PRD
//! that implements a primitive flips one row's `implemented` and its hooks.

use rmcp::ErrorData as McpError;
use rmcp::model::{ErrorCode, ServerCapabilities};
use serde_json::{Value, json};

/// The help code every unimplemented-capability `-32601` carries.
pub const UNSUPPORTED_CODE: &str = "capability_unsupported";

/// One MCP primitive family.
pub struct Capability {
    pub name: &'static str,
    /// Every JSON-RPC method of this family a client can send this host.
    pub methods: &'static [&'static str],
    pub implemented: bool,
    pub list_changed: bool,
    pub subscribe: bool,
    /// Name of the plan/limit ceiling that bounds this primitive, if any.
    pub ceiling: Option<&'static str>,
    /// The name synthorg's conformance probe table knows this family by.
    pub probe: &'static str,
    /// The tool to use instead while the capability is unimplemented.
    pub alternative: Option<&'static str>,
    pub help_code: &'static str,
}

pub const CAPABILITIES: &[Capability] = &[
    Capability {
        name: "tools",
        methods: &["tools/list", "tools/call"],
        implemented: true,
        list_changed: true,
        subscribe: false,
        ceiling: None,
        probe: "tools",
        alternative: None,
        help_code: UNSUPPORTED_CODE,
    },
    Capability {
        name: "resources",
        methods: &[
            "resources/list",
            "resources/read",
            "resources/templates/list",
            "resources/subscribe",
            "resources/unsubscribe",
        ],
        implemented: false,
        list_changed: false,
        subscribe: false,
        ceiling: None,
        probe: "resources",
        alternative: Some("host.table.list"),
        help_code: UNSUPPORTED_CODE,
    },
    Capability {
        name: "prompts",
        methods: &["prompts/list", "prompts/get"],
        implemented: true,
        list_changed: true,
        subscribe: false,
        ceiling: None,
        probe: "prompts",
        alternative: Some("host.quickstart"),
        help_code: UNSUPPORTED_CODE,
    },
    Capability {
        name: "completions",
        methods: &["completion/complete"],
        implemented: false,
        list_changed: false,
        subscribe: false,
        ceiling: None,
        probe: "completions",
        alternative: Some("host.help.search"),
        help_code: UNSUPPORTED_CODE,
    },
    Capability {
        name: "sampling",
        methods: &["sampling/createMessage"],
        implemented: false,
        list_changed: false,
        subscribe: false,
        ceiling: None,
        probe: "sampling",
        alternative: None,
        help_code: UNSUPPORTED_CODE,
    },
    Capability {
        name: "elicitation",
        methods: &["elicitation/create"],
        implemented: true,
        list_changed: false,
        subscribe: false,
        ceiling: None,
        probe: "elicitation",
        alternative: Some("host.ask.create"),
        help_code: UNSUPPORTED_CODE,
    },
    // `logging` and `roots` are deprecated by SEP-2577 in rmcp 3.3.0:
    // registered so the table is complete, never built.
    Capability {
        name: "logging",
        methods: &["logging/setLevel"],
        implemented: false,
        list_changed: false,
        subscribe: false,
        ceiling: None,
        probe: "logging",
        alternative: None,
        help_code: UNSUPPORTED_CODE,
    },
    Capability {
        name: "roots",
        methods: &["roots/list"],
        implemented: false,
        list_changed: false,
        subscribe: false,
        ceiling: None,
        probe: "roots",
        alternative: None,
        help_code: UNSUPPORTED_CODE,
    },
];

/// Rows that name a CLIENT capability this host consumes (it sends
/// `elicitation/create` when the client advertised it): implemented, but
/// never part of `ServerCapabilities`.
pub const CLIENT_SIDE: &[&str] = &["elicitation"];

/// The row that owns `method`, if any.
pub fn capability_for_method(method: &str) -> Option<&'static Capability> {
    CAPABILITIES.iter().find(|c| c.methods.contains(&method))
}

/// `-32601` for a method of an unimplemented capability, with the
/// `capability_unsupported` data shape and (when `base_url` is known) a
/// `help_url` like every other documented code.
pub fn unsupported_error(method: &str, base_url: Option<&str>) -> McpError {
    let cap = capability_for_method(method);
    let mut data = serde_json::Map::new();
    data.insert("code".into(), json!(UNSUPPORTED_CODE));
    data.insert("capability".into(), json!(cap.map(|c| c.name)));
    data.insert("method".into(), json!(method));
    data.insert("alternative".into(), json!(cap.and_then(|c| c.alternative)));
    if let Some(base) = base_url
        && let Some(url) = crate::help::help_url(base, cap.map_or(UNSUPPORTED_CODE, |c| c.help_code))
    {
        data.insert("help_url".into(), json!(url));
    }
    McpError::new(
        ErrorCode::METHOD_NOT_FOUND,
        format!("{method}: this host does not implement the `{}` capability", cap.map_or("?", |c| c.name)),
        Some(Value::Object(data)),
    )
}

/// `ServerCapabilities` for exactly the rows with `implemented: true`.
/// A row name this function cannot express panics (and the unit test below
/// walks every row), so a new row cannot silently advertise nothing.
pub fn server_capabilities() -> ServerCapabilities {
    let mut caps = ServerCapabilities::default();
    for row in CAPABILITIES.iter().filter(|c| c.implemented && !CLIENT_SIDE.contains(&c.name)) {
        match row.name {
            "tools" => {
                let b = ServerCapabilities::builder().enable_tools();
                caps.tools = if row.list_changed {
                    b.enable_tool_list_changed().build().tools
                } else {
                    b.build().tools
                };
            }
            "prompts" => {
                let b = ServerCapabilities::builder().enable_prompts();
                caps.prompts = if row.list_changed {
                    b.enable_prompts_list_changed().build().prompts
                } else {
                    b.build().prompts
                };
            }
            "resources" => {
                let b = ServerCapabilities::builder().enable_resources();
                caps.resources = match (row.list_changed, row.subscribe) {
                    (true, true) => b.enable_resources_list_changed().enable_resources_subscribe().build().resources,
                    (true, false) => b.enable_resources_list_changed().build().resources,
                    (false, true) => b.enable_resources_subscribe().build().resources,
                    (false, false) => b.build().resources,
                };
            }
            "completions" => caps.completions = Some(serde_json::Map::new()),
            "logging" => caps.logging = Some(serde_json::Map::new()),
            other => panic!("capabilities.rs: no ServerCapabilities mapping for implemented row `{other}`"),
        }
    }
    caps
}

/// One row as it appears under `primitives` in `GET /capabilities.json`.
fn primitive_json(c: &Capability) -> Value {
    json!({
        "implemented": c.implemented,
        "list_changed": c.list_changed,
        "subscribe": c.subscribe,
        "ceiling": c.ceiling,
        "probe": c.probe,
        "alternative": c.alternative,
        "methods": c.methods,
    })
}

/// `GET /capabilities.json`: `{version, primitives: {name: {...}}}` rendered
/// from [`CAPABILITIES`]; `only_implemented` is `?implemented=1`.
pub fn capabilities_document(only_implemented: bool) -> Value {
    let mut primitives = serde_json::Map::new();
    for c in CAPABILITIES.iter().filter(|c| c.implemented || !only_implemented) {
        primitives.insert(c.name.to_string(), primitive_json(c));
    }
    json!({"version": env!("CARGO_PKG_VERSION"), "primitives": primitives})
}

/// Names of the implemented rows, registry order.
pub fn implemented_names() -> Vec<&'static str> {
    CAPABILITIES.iter().filter(|c| c.implemented).map(|c| c.name).collect()
}

/// Names of the unimplemented rows, registry order.
pub fn unsupported_names() -> Vec<&'static str> {
    CAPABILITIES.iter().filter(|c| !c.implemented).map(|c| c.name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implemented_rows_render_tools_and_prompts_with_list_changed() {
        let v = serde_json::to_value(server_capabilities()).unwrap();
        assert_eq!(v, json!({"tools": {"listChanged": true}, "prompts": {"listChanged": true}}));
    }

    #[test]
    fn every_row_has_methods_and_a_known_method_owner() {
        for row in CAPABILITIES {
            assert!(!row.methods.is_empty(), "row `{}` has no methods", row.name);
            for m in row.methods {
                assert_eq!(capability_for_method(m).unwrap().name, row.name);
            }
        }
    }
}
