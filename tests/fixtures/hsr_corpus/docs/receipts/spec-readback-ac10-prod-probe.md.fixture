# Receipt — mcphost-shared-tool-spec-readback AC10: what prod is still missing

AC10's Given is "prod mcphost after deploy". This receipt records the one
thing that clause is still waiting on, as a fact a reader can re-derive
rather than a claim they have to take on trust.

## What was done

One read-only call against production, with **no `Authorization` header at
all** — `tools/list` is a listing call, and nothing on prod was created,
changed or deleted:

```
curl -s -X POST https://mcphost.dev/mcp \
  -H 'content-type: application/json' \
  -H 'accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/list"}'
```

## What it shows

1. **The deploy is the blocker.** Prod serves every step of AC10's
   sequence today — `signup`, `host.whoami`, `host.tool_publish`,
   `host.group.create`, `host.group.add`, `host.tool_share`, `host.usage` —
   but not `host.tool_spec_shared`, and its `host.tool_share` has no
   `expose_spec` property. The gap is exactly this branch's surface, and
   closing it is a deploy: an operator action (mcphost-deploy), not a
   build-agent one.
2. **Credentials are not a blocker.** Prod's `signup` is unauthenticated,
   so the live leg needs no operator-held tenant key:
   `MCPHOST_TENANT_A_KEY` / `MCPHOST_TENANT_B_KEY` are optional, and with
   them unset the `MCPHOST_LIVE=1` run signs its own two tenants up on the
   live endpoint.

`tests/mcphost_shared_tool_spec_readback_ac10_live_two_tenant_spec_read_trailer.rs`
reads the JSON below back on every run
(`only_the_deploy_separates_this_branch_from_prods_tool_surface`) and
compares it with the identical unauthenticated `tools/list` against a real
server built from this branch. When the deploy lands, that test and
`the_prod_probe_the_justification_cites_exists_and_shows_the_blocker` both
go red — which is precisely when AC10's prod leg stops being deferred and
has to be run for real.

## The probe

```json
{
  "_what": "A read-only probe of production mcphost's public, unauthenticated tools/list, captured to pin exactly what PRD-mcphost-shared-tool-spec-readback AC10's prod leg is still waiting on. Nothing was created, changed or deleted on prod: tools/list is a listing call and was made with no Authorization header at all.",
  "_why": "AC10's deferral has to name a blocker a reader can check, not a claim. This snapshot shows (a) prod does not yet serve host.tool_spec_shared and host.tool_share has no expose_spec property -- i.e. this branch is not deployed, the one operator action the live leg waits on -- and (b) prod's signup is unauthenticated, so the live leg needs no operator-held tenant credential.",
  "_reproduce": "curl -s -X POST https://mcphost.dev/mcp -H 'content-type: application/json' -H 'accept: application/json, text/event-stream' -d '{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}'",
  "probed_at": "2026-09-27T20:46:18Z",
  "endpoint": "https://mcphost.dev/mcp",
  "method": "tools/list",
  "authorization_header_sent": false,
  "raw_response_sha256": "48e998c7668d1c6d861811210a9386de6479461bedcc66acd7d356eec8e2a419",
  "raw_response_bytes": 95590,
  "tool_count": 135,
  "serves_host_tool_spec_shared": false,
  "host_tool_share_input_properties": [
    "description",
    "group",
    "name",
    "tenant_key",
    "visibility"
  ],
  "signup_entry": {
    "name": "signup",
    "description": "Create a tenant and receive a bearer key and namespace. Unauthenticated. Recommended: pass handoff: true to receive a short-lived, single-use handoff_token instead of the raw key -- redeem it once with host.redeem to get the key, so a transcript of this call and the redeem call, if it leaks, carries a dead credential. The raw-key path (handoff omitted) stays fully supported.",
    "inputSchema": {
      "properties": {
        "handoff": {
          "description": "Recommended: true to receive a handoff_token (redeem via host.redeem) instead of the raw key. Default false (raw key, unchanged).",
          "type": "boolean"
        },
        "name": {
          "description": "display name",
          "type": "string"
        }
      },
      "required": [
        "name"
      ],
      "type": "object"
    }
  },
  "host_tool_share_entry": {
    "name": "host.tool_share",
    "description": "Share one of this tenant's published tools with everyone (visibility: \"public\") or with a named group this tenant owns (visibility: \"group\", group: <name>). The tool keeps running in this tenant's own sandbox with this tenant's own secrets; a caller reaches it as <this tenant's namespace>.<name>.",
    "inputSchema": {
      "properties": {
        "description": {
          "description": "Catalog-facing blurb; shown by host.catalog.search/get.",
          "type": "string"
        },
        "group": {
          "description": "Required when visibility is \"group\"; must already exist (host.group.create).",
          "type": "string"
        },
        "name": {
          "description": "Local name of the tool to share.",
          "type": "string"
        },
        "tenant_key": {
          "description": "The key `signup` returned. Required only when this connection carries no Authorization: Bearer header -- when both are present, the header wins.",
          "type": "string"
        },
        "visibility": {
          "description": "\"public\" or \"group\".",
          "type": "string"
        }
      },
      "required": [
        "name",
        "visibility"
      ],
      "type": "object"
    }
  },
  "tool_names": [
    "billing.checkout",
    "billing.plans",
    "billing.status",
    "host.agent.contact_accept",
    "host.agent.contact_deny",
    "host.agent.contact_request",
    "host.agent.contacts",
    "host.agent.contacts_import",
    "host.agent.lookup",
    "host.agent.mute",
    "host.agent.profile_set",
    "host.agent.search",
    "host.agent.unmute",
    "host.agent.whoami",
    "host.bridge_test",
    "host.catalog.get",
    "host.catalog.search",
    "host.changelog",
    "host.channel.close",
    "host.channel.freeze",
    "host.channel.open",
    "host.channel.post",
    "host.channel.read",
    "host.channel.unfreeze",
    "host.docs.delete",
    "host.docs.get",
    "host.docs.index_config",
    "host.docs.list",
    "host.docs.purge",
    "host.docs.put",
    "host.docs.reindex",
    "host.docs.search",
    "host.docs.status",
    "host.enduser.assertion_secret_rotate",
    "host.enduser.audit",
    "host.enduser.export",
    "host.enduser.get",
    "host.enduser.list",
    "host.enduser.purge",
    "host.enduser.revoke",
    "host.enduser.unrevoke",
    "host.enduser.whoami",
    "host.export",
    "host.group.add",
    "host.group.create",
    "host.group.list",
    "host.group.remove",
    "host.key_rotate",
    "host.msg.ack",
    "host.msg.block",
    "host.msg.inbox",
    "host.msg.reply",
    "host.msg.send",
    "host.msg.thread",
    "host.msg.unblock",
    "host.msg.wait",
    "host.oauth.audit",
    "host.oauth.audit_export",
    "host.oauth.client_approve",
    "host.oauth.client_deny",
    "host.oauth.doctor",
    "host.oauth.grant_revoke",
    "host.oauth.grants",
    "host.oauth.issuer_remove",
    "host.oauth.issuer_set",
    "host.oauth.issuers",
    "host.oauth.pending",
    "host.oauth.policy",
    "host.oauth.policy_set",
    "host.oauth.provider",
    "host.oauth.provider_remove",
    "host.oauth.provider_set",
    "host.oauth.revoke_all",
    "host.oauth.scope_set",
    "host.oauth.scopes",
    "host.oauth.trusted_issuer_remove",
    "host.oauth.trusted_issuer_set",
    "host.oauth.trusted_issuers",
    "host.progress",
    "host.quickstart",
    "host.redeem",
    "host.registry_publish",
    "host.runs.cancel",
    "host.runs.get",
    "host.runs.list",
    "host.runs.part",
    "host.runs.purge",
    "host.runs.wait",
    "host.secret_list",
    "host.secret_set",
    "host.self_offboard",
    "host.share.caller_limit",
    "host.share.caller_limit_remove",
    "host.state.delete",
    "host.state.delete_rows",
    "host.state.get",
    "host.state.insert",
    "host.state.list",
    "host.state.query",
    "host.state.set",
    "host.state.table_create",
    "host.state.table_drop",
    "host.table.append",
    "host.table.create",
    "host.table.describe",
    "host.table.drop",
    "host.table.list",
    "host.table.model_set",
    "host.table.models",
    "host.table.query",
    "host.table.schema",
    "host.tool_call",
    "host.tool_diff",
    "host.tool_history",
    "host.tool_list",
    "host.tool_logs",
    "host.tool_publish",
    "host.tool_remove",
    "host.tool_rollback",
    "host.tool_run",
    "host.tool_share",
    "host.tool_test",
    "host.tool_unshare",
    "host.trigger.fire",
    "host.trigger.get",
    "host.trigger.list",
    "host.trigger.pause",
    "host.trigger.remove",
    "host.trigger.replay",
    "host.trigger.resume",
    "host.trigger.set",
    "host.trigger.test",
    "host.usage",
    "host.whoami",
    "signup"
  ]
}
```
