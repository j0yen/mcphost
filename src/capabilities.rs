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

/// First protocol revision whose Streamable HTTP transport requires HTTP 404
/// for a method the server does not implement.
pub const NOT_FOUND_STATUS_FROM: &str = "2026-07-28";

/// The `_meta` key a 2026-07-28 request carries its protocol version under.
pub const META_PROTOCOL_VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";

/// Methods this host dispatches that rmcp 3.3.0 refuses from
/// [`NOT_FOUND_STATUS_FROM`] on ("legacy-only": `subscriptions/listen` replaces
/// the two resource subscription methods, and `ping` is gone).
pub const LEGACY_ONLY_METHODS: &[&str] = &["ping", "resources/subscribe", "resources/unsubscribe"];

/// The [`CORE_METHODS`] rmcp routes but this host's handler leaves at rmcp's
/// default method-not-found, so the allowlist omits them (the R9c drift test
/// fails when one starts answering, or when a listed one stops).
pub const CORE_NOT_HANDLED: &[&str] = &["subscriptions/listen", "tasks/get", "tasks/update", "tasks/cancel"];

crate::err_code! {
    pub METHOD_UNKNOWN,
    code: "method_unknown", family: Spec, class: AgentFault, retryable: false,
    remedy: "Do not post this method: the host does not dispatch it on this protocol revision. Read /capabilities.json for the methods it implements.",
}

crate::err_code! {
    pub METHOD_SERVER_TO_CLIENT,
    code: "method_server_to_client", family: Spec, class: AgentFault, retryable: false,
    remedy: "Do not post this method: the host sends it to the client. Answer it when the host sends it, or read /capabilities.json server_to_client.",
}

/// One MCP primitive family.
pub struct Capability {
    pub name: &'static str,
    /// Every JSON-RPC method of this family a client can send this host.
    pub methods: &'static [&'static str],
    /// The subset of `methods` this host SENDS to the client (a client
    /// posting one is refused even when the row is implemented).
    pub server_to_client: &'static [&'static str],
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
    /// The synthorg conformance cell that proves this row's `list_changed`
    /// delivery. Every row with `list_changed: true` names one.
    pub proving_cell: Option<&'static str>,
}

/// Methods every client may send that belong to no primitive row: the
/// protocol core plus the rmcp-routed lifecycle methods (`server/discover`,
/// `subscriptions/listen`, `tasks/*`) this registry has no primitive row for.
/// `tests/cgd_ac04_dispatch_table_matches_registry.rs` ties this list and the
/// rows to rmcp's `ClientRequest` dispatch table.
pub const CORE_METHODS: &[&str] = &[
    "initialize",
    "ping",
    "tools/list",
    "tools/call",
    "server/discover",
    "subscriptions/listen",
    "tasks/get",
    "tasks/update",
    "tasks/cancel",
];

/// The synthorg conformance cell that proves `subscriptions/listen` delivers
/// list changes (PRD-mcphost-subscriptions-listen R11b).
pub const PROVING_CELL_LISTEN: &str = "subscriptions_listen";

pub const CAPABILITIES: &[Capability] = &[
    Capability {
        name: "tools",
        methods: &["tools/list", "tools/call"],
        server_to_client: &[],
        implemented: true,
        list_changed: true,
        subscribe: false,
        ceiling: None,
        probe: "tools",
        alternative: None,
        help_code: UNSUPPORTED_CODE,
        proving_cell: Some(PROVING_CELL_LISTEN),
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
        server_to_client: &[],
        implemented: true,
        list_changed: false,
        subscribe: true,
        ceiling: Some("resources_max"),
        probe: "resources",
        alternative: Some("host.table.list"),
        help_code: UNSUPPORTED_CODE,
        proving_cell: None,
    },
    Capability {
        name: "prompts",
        methods: &["prompts/list", "prompts/get"],
        server_to_client: &[],
        implemented: true,
        list_changed: true,
        subscribe: false,
        ceiling: None,
        probe: "prompts",
        alternative: Some("host.quickstart"),
        help_code: UNSUPPORTED_CODE,
        proving_cell: Some(PROVING_CELL_LISTEN),
    },
    Capability {
        name: "completions",
        methods: &["completion/complete"],
        server_to_client: &[],
        implemented: false,
        list_changed: false,
        subscribe: false,
        ceiling: None,
        probe: "completions",
        alternative: Some("host.help.search"),
        help_code: UNSUPPORTED_CODE,
        proving_cell: None,
    },
    Capability {
        name: "sampling",
        methods: &["sampling/createMessage"],
        server_to_client: &["sampling/createMessage"],
        implemented: crate::inband::in_band_available(),
        list_changed: false,
        subscribe: false,
        ceiling: Some("sampling_tokens_per_day"),
        probe: "sampling",
        alternative: None,
        help_code: UNSUPPORTED_CODE,
        proving_cell: None,
    },
    Capability {
        name: "elicitation",
        methods: &["elicitation/create"],
        server_to_client: &["elicitation/create"],
        implemented: crate::inband::in_band_available(),
        list_changed: false,
        subscribe: false,
        ceiling: None,
        probe: "elicitation",
        alternative: Some("host.ask.create"),
        help_code: UNSUPPORTED_CODE,
        proving_cell: None,
    },
    // `logging` and `roots` are deprecated by SEP-2577 in rmcp 3.3.0:
    // registered so the table is complete, never built.
    Capability {
        name: "logging",
        methods: &["logging/setLevel"],
        server_to_client: &[],
        implemented: false,
        list_changed: false,
        subscribe: false,
        ceiling: None,
        probe: "logging",
        alternative: None,
        help_code: UNSUPPORTED_CODE,
        proving_cell: None,
    },
    Capability {
        name: "roots",
        methods: &["roots/list"],
        server_to_client: &["roots/list"],
        implemented: false,
        list_changed: false,
        subscribe: false,
        ceiling: None,
        probe: "roots",
        alternative: None,
        help_code: UNSUPPORTED_CODE,
        proving_cell: None,
    },
];

/// Rows that name a CLIENT capability this host consumes (it sends
/// `elicitation/create` / `sampling/createMessage` when the client advertised
/// it): implemented, but never part of `ServerCapabilities`.
pub const CLIENT_SIDE: &[&str] = &["elicitation", "sampling"];

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

/// The error the pre-dispatch gate answers for `method`, if it is refusable:
/// a method of an unimplemented row.
pub fn refusal(method: &str, base_url: Option<&str>) -> Option<McpError> {
    let cap = capability_for_method(method)?;
    if !cap.implemented {
        Some(unsupported_error(method, base_url))
    } else if cap.server_to_client.contains(&method) {
        Some(server_to_client_error(method, cap, base_url))
    } else {
        None
    }
}

/// PRD-mcphost-unimplemented-method-status R9: the HTTP status of a `-32601`
/// refusal. 404 from 2026-07-28 on (the transport's "Protocol Version Header"
/// section); 200 on every earlier revision, where a 404 inside a session tells
/// the client to drop it, and when the revision is unknown. Keyed on the
/// protocol version alone.
pub fn refusal_status(protocol_version: Option<&str>) -> http::StatusCode {
    match protocol_version {
        Some(v) if v >= NOT_FOUND_STATUS_FROM => http::StatusCode::NOT_FOUND,
        _ => http::StatusCode::OK,
    }
}

/// True when the host dispatches `method` on `protocol_version`: the generated
/// allowlist, [`CORE_METHODS`] plus the client-to-server methods of implemented
/// rows, minus [`LEGACY_ONLY_METHODS`] from [`NOT_FOUND_STATUS_FROM`] on.
pub fn dispatches(method: &str, protocol_version: Option<&str>) -> bool {
    if refusal_status(protocol_version) == http::StatusCode::NOT_FOUND && LEGACY_ONLY_METHODS.contains(&method) {
        return false;
    }
    (CORE_METHODS.contains(&method) && !CORE_NOT_HANDLED.contains(&method))
        || capability_for_method(method).is_some_and(|c| c.implemented && !c.server_to_client.contains(&method))
}

/// `-32601` for a method outside the allowlist that no refusal names.
pub fn unknown_method_error(method: &str, base_url: Option<&str>) -> McpError {
    let cap = capability_for_method(method);
    let mut data = serde_json::Map::new();
    data.insert("code".into(), json!(METHOD_UNKNOWN));
    data.insert("capability".into(), json!(cap.map(|c| c.name)));
    data.insert("method".into(), json!(method));
    data.insert("alternative".into(), json!(cap.and_then(|c| c.alternative)));
    if let Some(base) = base_url
        && let Some(url) = crate::help::help_url(base, METHOD_UNKNOWN)
    {
        data.insert("help_url".into(), json!(url));
    }
    McpError::new(ErrorCode::METHOD_NOT_FOUND, format!("{method}: method not found"), Some(Value::Object(data)))
}

/// The error the pre-dispatch gate answers for `method` on `protocol_version`:
/// the registry's [`refusal`], else [`unknown_method_error`] for any method the
/// allowlist omits. `None` for a method the host dispatches.
pub fn gate_error(method: &str, protocol_version: Option<&str>, base_url: Option<&str>) -> Option<McpError> {
    refusal(method, base_url).or_else(|| (!dispatches(method, protocol_version)).then(|| unknown_method_error(method, base_url)))
}

/// `-32601` for a server-to-client method of an IMPLEMENTED row posted by a
/// client: the host sends it, it does not receive it.
fn server_to_client_error(method: &str, cap: &Capability, base_url: Option<&str>) -> McpError {
    let mut data = serde_json::Map::new();
    data.insert("code".into(), json!(METHOD_SERVER_TO_CLIENT));
    data.insert("capability".into(), json!(cap.name));
    data.insert("method".into(), json!(method));
    data.insert("alternative".into(), json!(cap.alternative));
    if let Some(base) = base_url
        && let Some(url) = crate::help::help_url(base, METHOD_SERVER_TO_CLIENT)
    {
        data.insert("help_url".into(), json!(url));
    }
    McpError::new(
        ErrorCode::METHOD_NOT_FOUND,
        format!("{method}: the host sends this request to the client; clients do not post it"),
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

/// PRD-mcphost-elicitation-in-band-on-stateless-transport R8: how a row that
/// names a client capability reaches the client, as a `transport` field of its
/// `/capabilities.json` row. Keyed by the registry row's name, so the
/// advertisement says exactly which clients get which path.
pub fn transport_note(name: &str) -> Option<&'static str> {
    (name == "elicitation")
        .then_some("in-band when the client echoes Mcp-Session-Id and advertises form; link otherwise")
}

/// One row as it appears under `primitives` in `GET /capabilities.json`.
fn primitive_json(c: &Capability) -> Value {
    let mut row = json!({
        "implemented": c.implemented,
        "list_changed": c.list_changed,
        "subscribe": c.subscribe,
        "ceiling": c.ceiling,
        "probe": c.probe,
        "alternative": c.alternative,
        "methods": c.methods,
        "server_to_client": c.server_to_client,
        "proving_cell": c.proving_cell,
    });
    if let Some(transport) = transport_note(c.name) {
        row["transport"] = json!(transport);
    }
    row
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
        assert_eq!(
            v,
            json!({
                "tools": {"listChanged": true},
                "prompts": {"listChanged": true},
                "resources": {"subscribe": true}
            })
        );
    }

    #[test]
    fn every_list_changed_row_names_a_proving_cell() {
        for row in CAPABILITIES.iter().filter(|c| c.list_changed) {
            assert!(row.proving_cell.is_some(), "row `{}` advertises list_changed with no proving_cell", row.name);
        }
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
