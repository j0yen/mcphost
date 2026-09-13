//! PRD-mcphost-client-attribution-default-leak
//! AC1 — Given a single-shot `tools/call` request with no prior
//! `initialize` and no `clientInfo` (a bare JSON-RPC POST, no
//! `_meta["io.modelcontextprotocol/clientInfo"]` field at all -- the exact
//! shape `stress_protocol.py` documents this host's streamable-HTTP
//! transport as tolerating), When the tenant record is read back
//! (`host.whoami`, and the underlying `tenants` row), Then
//! `client_name`/`client_version` are null, not `rmcp`/any SDK-version
//! string.
//!
//! Root cause (see `src/handler.rs`'s `peer_client_info` doc comment):
//! `rmcp`'s stateless streamable-HTTP path synthesizes a placeholder
//! `Implementation::default()` (== `rmcp`'s own crate name/version, e.g.
//! `"rmcp"`/`"3.2.0"`, baked in at `rmcp`'s own compile time) whenever a
//! request carries no real `clientInfo`, purely so `protocol_version()` has
//! a fallback -- and `RequestContext::client_info()` surfaced that same
//! placeholder as if it were genuine caller-reported attribution.

use crate::common;
use common::{McpClient, extract_structured};
use serde_json::{Value, json};

/// Send a genuinely bare JSON-RPC `tools/call` -- no `initialize`, no
/// `_meta`, no `clientInfo` anywhere, and no `MCP-Protocol-Version` header
/// declaring the 2026-07-28 draft (whose SEP-2575 `_meta` keys --
/// `protocolVersion`/`clientCapabilities`/`clientInfo` -- `rmcp` then makes
/// mandatory on every request, which a genuinely bare caller never sends;
/// `post_with_mcp_name_override` is the one existing helper that omits that
/// header while still setting the `Mcp-Method`/`Mcp-Name` headers `rmcp`
/// separately requires to match the body) -- and return the parsed
/// `result`. This is the exact "single-shot POST, no prior `initialize`"
/// shape `stress_protocol.py` documents as tolerated.
async fn bare_tools_call(client: &McpClient, name: &str, arguments: Value, id: u64) -> Value {
    let resp = client
        .post_with_mcp_name_override(
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": {"name": name, "arguments": arguments},
            }),
            name,
        )
        .await;
    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .unwrap_or_else(|e| panic!("parse {name} response ({status}): {e}"));
    assert!(
        body.get("error").is_none(),
        "bare {name} call must succeed (no clientInfo is not an error): {body:?}"
    );
    body.get("result").cloned().unwrap_or(Value::Null)
}

#[tokio::test]
async fn bare_signup_and_whoami_report_null_client_info_not_rmcp_sdk() {
    let server = common::TestServer::start().await;
    let anon = McpClient::new(&server.base_url);

    // Bare signup: no `initialize`, no `_meta`, no `clientInfo` field at
    // all -- distinct from every pre-existing attribution test, which all
    // ride `McpClient::call`'s always-on `_meta["io.modelcontextprotocol/
    // clientInfo"]` injection.
    let signup_result = bare_tools_call(
        &anon,
        "signup",
        json!({"name": "No ClientInfo Agent"}),
        1,
    )
    .await;
    let structured = extract_structured(&signup_result);
    let ns = structured["tenant"]
        .as_str()
        .expect("signup result carries tenant namespace")
        .to_string();
    let key = structured["key"]
        .as_str()
        .expect("signup result carries key")
        .to_string();

    // Read back straight from the DB -- the deepest-level assertion the
    // PRD asks for, independent of any surface's own JSON shaping.
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("query")
        .expect("tenant exists");
    assert_eq!(
        tenant.client_name, None,
        "no clientInfo was sent -- client_name must be null, not fabricated: {:?}",
        tenant.client_name
    );
    assert_eq!(
        tenant.client_version, None,
        "no clientInfo was sent -- client_version must be null, not fabricated: {:?}",
        tenant.client_version
    );
    assert_ne!(
        tenant.client_name.as_deref(),
        Some("rmcp"),
        "must never leak rmcp's own SDK identity as caller attribution"
    );

    // Read back via `host.whoami` too -- P0 requirement 2: every surface
    // reading this column must see the same (corrected) value, not a
    // re-derived string.
    let authed = McpClient::with_bearer(&server.base_url, &key);
    let whoami_result = bare_tools_call(&authed, "host.whoami", json!({}), 2).await;
    let whoami = extract_structured(&whoami_result);
    assert_eq!(
        whoami["client_name"],
        Value::Null,
        "host.whoami must report null client_name for a handshake-skipping caller: {whoami:?}"
    );
    assert_eq!(
        whoami["client_version"],
        Value::Null,
        "host.whoami must report null client_version for a handshake-skipping caller: {whoami:?}"
    );
}
