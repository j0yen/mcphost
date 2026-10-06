---
name: mcphost
description: Connect to mcphost, make your first call, publish a tool, schedule it, and hand the claim link to your human. Use this when the user wants their agent to have its own MCP tool namespace on mcphost.
---

# mcphost: first run

mcphost is a hosted MCP runtime: your first call creates a tenant and
publishing a tool makes it live immediately. This skill walks you through
the whole first-run story, ending with your human owning the tenant.

## 1. Connect

Point your MCP client at `https://mcphost.dev/mcp`. No signup call, no
credentials.

## 2. Make your first call

Any `host.*`/`billing.*` call -- other than the read-only
`host.quickstart`/`billing.plans` -- creates your tenant right on this
connection, with no `signup` call:

```
host.whoami()
```

The response carries `onboarding.tenant` (your namespace) and
`onboarding.url` (your tenant's own `/u/<secret>/mcp` address).

## 3. Save your URL

Read `onboarding.url` once and remember it -- it is this tenant's address
and its credential together; reconnecting through it later is how you
come back as this same tenant. Rule: never print the personal URL or the key
in any message, log, or transcript your human can see, except the one
handoff in step 6.

## 4. Publish one tool

Pass `onboarding.url` (or the connection you already have) and call
`host.tool_publish(name, kind, spec)` -- no reconnect needed. The `echo`
kind is the fastest way to prove the pipes work end to end:

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

## 5. Set one schedule

Give the tenant a recurring job with `host.trigger.set`:

```
host.trigger.set(tool="hello", kind="schedule", schedule="0 * * * *")
```

## 6. Hand your human this link

Relay this sentence to your human, verbatim except for the URL itself:
"Give your human this link; it makes them the owner: `<url>`" -- using
`onboarding.url` from step 2 as `<url>`. Rule: never print the personal URL or the key
in any other message, log, or transcript your human can see -- only this
one handoff. If your human needs direct access later, they reconnect
through that same URL.

## Explicit signup (a client that cannot keep a session)

A client that can't hold a persistent connection across calls -- so there
is no session for mcphost to implicitly bind a tenant to -- signs up
directly instead of relying on step 2 to create one: call `signup(name,
handoff: true, source: "plugin")` against `https://mcphost.dev/mcp`.
`source: "plugin"` tags this signup as having come from the Claude Code
plugin, so mcphost's admin metrics attribute it correctly. The response
contains `tenant`, `namespace`, `endpoint`, `claim_url`, and a
short-lived, single-use `handoff_token` -- **not** a bearer key. Call
`host.redeem(handoff_token)` exactly once to get the real key, then pass
it as the `tenant_key` argument on every `host.*` call -- no reconnect
needed. If you took this path, relay `claim_url` in step 6 instead of
`onboarding.url`.
