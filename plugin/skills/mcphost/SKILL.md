---
name: mcphost
description: Connect to mcphost, sign up in handoff mode, publish a tool, schedule it, and hand the claim link to your human. Use this when the user wants their agent to have its own MCP tool namespace on mcphost.
---

# mcphost: first run

mcphost is a hosted MCP runtime: sign up with one tool call, publish a
tool with the next, and it is live immediately. This skill walks you
through the whole first-run story, ending with your human owning the
tenant.

## 1. Sign up in handoff mode

Call `signup(name, handoff: true, source: "plugin")` against
`https://mcphost.dev/mcp`. `source: "plugin"` tags this signup as having
come from the Claude Code plugin, so mcphost's admin metrics attribute it
correctly. The response contains `tenant`, `namespace`, `endpoint`,
`claim_url`, and a short-lived, single-use `handoff_token` -- **not** a
bearer key.

## 2. Redeem the handoff token

Call `host.redeem(handoff_token)` exactly once. It returns the tenant's
real bearer `key`. The token is now dead, so nothing in this
transcript still carries a live credential except the response to this
one call.

## 3. Publish one tool

Pass the `key` from step 2 as the `tenant_key` argument and call
`host.tool_publish(name, kind, spec)` on the same connection -- no
reconnect needed. The `echo` kind is the fastest way to prove the pipes
work end to end:

```
host.tool_publish(name="hello", kind="echo", spec={"schema": {"type": "object"}})
```

If you publish a `python` tool instead, its own code reaches this tenant's
data with `import mcphost`:

<!-- mcphost-sandbox-api:start -->
A python tool's own code does `import mcphost` to reach this tenant's data without a second tool call:

- `mcphost.state` -- per-tenant key/value store and filter-grammar tables (same store host.state.* uses)
- `mcphost.table` -- per-tenant SQL tables (same store host.table.* uses)
- `mcphost.docs` -- read or search this tenant's stored documents (same store host.docs.* uses)
- `mcphost.lineage` -- trace an artifact's lineage or estimate a change's blast radius (same report host.lineage.* uses)
- `mcphost.drift` -- list drift review items (same list host.drift.reviews returns)
- `mcphost.channel` -- post to or read a tenant channel (same store host.channel.* uses)
- `mcphost.msg` -- send or read direct messages (same store host.msg.* uses)
<!-- mcphost-sandbox-api:end -->

## 4. Set one schedule

Give the tenant a recurring job with `host.trigger.set`:

```
host.trigger.set(tool="hello", kind="schedule", schedule="0 * * * *")
```

## 5. Hand the claim link to your human

Relay this sentence to your human, verbatim except for the URL itself:
"Claim this backend so it belongs to you: `<claim_url>` (link expires in 7
days)." Rule: never print the key in any message, log, or transcript your
human can see -- the bearer key from step 2 stays inside your own tool
calls only. If your human needs direct access later, they claim the
tenant at `claim_url` and manage it from there.
