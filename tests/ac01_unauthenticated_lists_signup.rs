//! AC1 — Given a fresh server with an empty data directory, When an
//! unauthenticated client sends `initialize` then `tools/list`, Then the
//! response carries `MCP-Protocol-Version` and lists `signup` alongside
//! the discoverable `host.*` control plane (PRD-mcphost-session-key
//! requirement 1 widened this from `signup` alone), and no `admin.*` tool
//! or namespaced tenant tool.

use crate::common;
use common::{McpClient, TestServer};
use rmcp::model::ProtocolVersion;
use serde_json::Value;

#[tokio::test]
async fn unauthenticated_tools_list_is_signup_only() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let init_resp = client
        .post_raw(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2026-07-28",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0.1"}
            }
        }))
        .await;
    // PRD-mcphost-protocol-compat requirement 6: mcphost advertises the
    // version it actually negotiates (`rmcp::model::ProtocolVersion::LATEST`),
    // not the `2026-07-28` literal this assertion used to hardcode -- that
    // literal *was* defect B (see AC9/AC10 in compat_ac08_ac09_ac10_advertised_version.rs).
    assert_eq!(
        init_resp
            .headers()
            .get("MCP-Protocol-Version")
            .and_then(|v| v.to_str().ok()),
        Some(ProtocolVersion::LATEST.as_str()),
        "every response must carry MCP-Protocol-Version"
    );
    let init_body: Value = init_resp.json().await.expect("parse initialize");
    assert!(
        init_body.get("error").is_none(),
        "initialize should not error: {init_body:?}"
    );

    let tools = client
        .tools_list()
        .await
        .expect("tools/list should succeed unauthenticated");
    let tool_names: Vec<&str> = tools["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        tool_names.contains(&"signup"),
        "unauthenticated tools/list must list signup: {tool_names:?}"
    );
    for expected in [
        "host.whoami",
        // PRD-mcphost-handoff-token requirement 2/3: the handoff exchange
        // and rotation surfaces are discoverable unauthenticated too, same
        // as every other host.*-style tool below (host.redeem's own call
        // works unauthenticated -- the token is the proof; host.key_rotate
        // still requires the caller's current key to actually invoke, same
        // gating every other host.* descriptor here already has).
        "host.redeem",
        "host.key_rotate",
        // PRD-mcphost-tenant-self-offboard P0 requirement 1 / AC1-4:
        // discoverable unauthenticated too, same as host.key_rotate above --
        // authenticated to CALL (host_schema's tenant_key is the credential
        // being retired), not to discover.
        "host.self_offboard",
        "host.tool_publish",
        // PRD-mcphost-publish-first-try requirement 4: discoverable
        // unauthenticated too, same as the rest of the host.* control
        // plane -- an agent that hasn't signed up yet still gets the
        // signup-first step from it.
        "host.quickstart",
        "host.tool_list",
        "host.tool_remove",
        "host.tool_logs",
        "host.tool_test",
        // PRD-mcphost-rest-bridge P1 requirement: "test before deploy" for
        // an unpublished spec, discoverable unauthenticated too, same as
        // the rest of the host.* control plane.
        "host.bridge_test",
        // PRD-mcphost-code-tools-warm-pool requirement 3: the debug-run RPC
        // is discoverable unauthenticated too, same as the rest of the
        // host.* control plane.
        "host.tool_run",
        "host.tool_call",
        "host.usage",
        // PRD-mcphost-tenant-data-export P0 requirements 1-3: the export
        // job is discoverable unauthenticated too, same as every other
        // host.*-style tool above.
        "host.export",
        "host.secret_set",
        "host.secret_list",
        "host.registry_publish",
        // PRD-mcphost-tenant-state requirement 2: the host.state.* control
        // plane is discoverable unauthenticated too, same as every other
        // host.*-style tool above.
        "host.state.get",
        "host.state.set",
        "host.state.delete",
        "host.state.list",
        "host.state.table_create",
        "host.state.table_drop",
        "host.state.insert",
        "host.state.query",
        "host.state.delete_rows",
        // PRD-mcphost-tenant-tables requirement 1: the real-SQL table
        // control plane is discoverable unauthenticated too, same as every
        // other host.*-style tool above.
        "host.table.create",
        "host.table.append",
        "host.table.query",
        "host.table.list",
        "host.table.drop",
        "host.table.schema",
        // PRD-mcphost-table-semantic-model requirement 4: the generated
        // semantic model is discoverable unauthenticated too, same as
        // every other host.*-style tool above.
        "host.table.describe",
        "host.table.model_set",
        "host.table.models",
        // PRD-mcphost-sharing requirement 1: the sharing/catalog control
        // plane is discoverable unauthenticated too, same as every other
        // host.*-style tool above.
        "host.tool_share",
        "host.tool_unshare",
        "host.group.create",
        "host.group.add",
        "host.group.remove",
        "host.group.list",
        "host.catalog.search",
        "host.catalog.get",
        // PRD-mcphost-runs-and-jobs requirement 7: the runs ledger's
        // tenant-facing tools are discoverable unauthenticated too, same
        // as every other host.*-style tool above.
        "host.runs.get",
        "host.runs.list",
        "host.runs.cancel",
        "host.runs.purge",
        "host.runs.wait",
        // PRD-mcphost-run-result-overflow-to-state requirements 3/4: an
        // oversized async result's counters/parts are discoverable
        // unauthenticated too, same as every other host.*-style tool above.
        "host.progress",
        "host.runs.part",
        // PRD-mcphost-schedules requirement 2: the schedule-trigger control
        // plane is discoverable unauthenticated too, same as every other
        // host.*-style tool above.
        "host.trigger.set",
        "host.trigger.list",
        "host.trigger.get",
        "host.trigger.pause",
        "host.trigger.resume",
        "host.trigger.remove",
        "host.trigger.fire",
        // PRD-mcphost-inbound-events P0 requirement 3: the event-trigger
        // dry-run/replay tools are discoverable unauthenticated too, same
        // as every other host.trigger.* tool above.
        "host.trigger.test",
        "host.trigger.replay",
        // PRD-grand-loop-billing AC1: billing.plans is anonymous-and-tenant
        // reachable, discoverable here for the same reason host.quickstart
        // is -- billing.status/billing.checkout are tenant-only in what
        // they return but their descriptors are discoverable pre-auth too,
        // same as every other host.*-style tool above.
        "billing.plans",
        "billing.status",
        "billing.checkout",
        // PRD-mcphost-agent-directory requirements 2-5: the agent-directory
        // control plane is discoverable unauthenticated too, same as every
        // other host.*-style tool above.
        "host.agent.whoami",
        "host.agent.profile_set",
        "host.agent.lookup",
        "host.agent.search",
        // PRD-mcphost-agent-inbox requirements 2-5, 9: the agent-inbox
        // messaging control plane is discoverable unauthenticated too,
        // same as every other host.*-style tool above.
        "host.msg.send",
        "host.msg.reply",
        "host.msg.inbox",
        "host.msg.thread",
        "host.msg.ack",
        "host.msg.block",
        "host.msg.unblock",
        // PRD-mcphost-agent-wake P0 requirement 6: host.msg.wait is
        // discoverable unauthenticated too, same as every other
        // host.msg.* tool above.
        "host.msg.wait",
        // PRD-mcphost-agent-consent requirements 2, 3, 5, 10: the consent
        // control plane (contacts/mutes layered on top of contact_policy)
        // is discoverable unauthenticated too, same as every other
        // host.*-style tool above.
        "host.agent.contact_request",
        "host.agent.contacts",
        "host.agent.contact_accept",
        "host.agent.contact_deny",
        "host.agent.mute",
        "host.agent.unmute",
        "host.agent.contacts_import",
        // PRD-mcphost-agent-mesh-ops / PRD-mcphost-agent-channels: the six
        // host.channel.* tools are discoverable unauthenticated too, same
        // as every other host.*-style tool above.
        "host.channel.open",
        "host.channel.post",
        "host.channel.read",
        "host.channel.close",
        "host.channel.freeze",
        "host.channel.unfreeze",
        // PRD-mcphost-document-store: the six host.docs.* tools are
        // discoverable unauthenticated too, same as every other host.*-style
        // tool above.
        "host.docs.put",
        "host.docs.get",
        "host.docs.list",
        "host.docs.delete",
        "host.docs.status",
        "host.docs.purge",
        // PRD-mcphost-oauth-resource-server requirement 3: discoverable
        // unauthenticated too, same as every other host.*-style tool
        // above -- registering an issuer is itself an authenticated
        // (tenant_key/bearer-gated) call, not the discovery of it.
        "host.oauth.issuer_set",
        "host.oauth.issuer_remove",
        "host.oauth.issuers",
        // PRD-mcphost-docs-semantic-search: the three host.docs.search/
        // index_config/reindex tools are discoverable unauthenticated too,
        // same as every other host.*-style tool above.
        "host.docs.search",
        "host.docs.index_config",
        "host.docs.reindex",
        // PRD-mcphost-end-user-identity: discoverable unauthenticated too,
        // same as every other host.*-style tool above -- both are
        // authenticated (tenant_key/bearer-gated) calls, not the discovery
        // of them.
        "host.enduser.whoami",
        "host.enduser.assertion_secret_rotate",
    ] {
        assert!(
            tool_names.contains(&expected),
            "unauthenticated tools/list must list {expected}: {tool_names:?}"
        );
    }
    assert!(
        !tool_names.iter().any(|n| n.starts_with("admin.")),
        "unauthenticated tools/list must not list any admin.* tool: {tool_names:?}"
    );
    assert_eq!(
        tool_names.len(),
        107,
        "signup + host.redeem + host.key_rotate (PRD-mcphost-handoff-token) + the thirteen \
         host.* tools (incl. host.quickstart, host.tool_run, and host.bridge_test) + \
         host.tool_call + the three host.tool_history/host.tool_rollback/host.tool_diff \
         tools (PRD-mcphost-tool-versions) + the nine host.state.* tools \
         (PRD-mcphost-tenant-state) \
         + the nine host.table.* tools (PRD-mcphost-tenant-tables, \
         PRD-mcphost-table-semantic-model) \
         + the eight host.tool_share/host.tool_unshare/host.group.*/host.catalog.* tools \
         (PRD-mcphost-sharing) + the five host.runs.* tools (PRD-mcphost-runs-and-jobs) \
         + the two host.progress/host.runs.part tools \
         (PRD-mcphost-run-result-overflow-to-state) \
         + the nine host.trigger.* tools (PRD-mcphost-schedules, PRD-mcphost-inbound-events) \
         + the three billing.* tools + the four host.agent.* tools \
         (PRD-mcphost-agent-directory) + the eight host.msg.* tools \
         (PRD-mcphost-agent-inbox, PRD-mcphost-agent-wake) + the seven \
         host.agent.contact_*/mute/unmute tools (PRD-mcphost-agent-consent) + \
         host.self_offboard (PRD-mcphost-tenant-self-offboard) + \
         host.changelog (PRD-mcphost-host-tool-deprecation) + \
         host.export (PRD-mcphost-tenant-data-export) + \
         the six host.channel.* tools (PRD-mcphost-agent-mesh-ops, \
         PRD-mcphost-agent-channels) + the six host.docs.* tools \
         (PRD-mcphost-document-store) + \
         the three host.oauth.* tools (PRD-mcphost-oauth-resource-server) + \
         the three host.docs.search/index_config/reindex tools \
         (PRD-mcphost-docs-semantic-search) + \
         the two host.enduser.* tools (PRD-mcphost-end-user-identity): \
         {tool_names:?}"
    );
}
