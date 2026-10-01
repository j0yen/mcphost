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

## 3b. Writing a `python` tool? `import mcphost`

A `python`-kind tool's own code reaches the sandbox API with one import:
`import mcphost`. The registered submodules are `mcphost.state`,
`mcphost.table`, `mcphost.docs`, `mcphost.lineage`; `mcphost.call` and
`mcphost.progress` are attributes on `mcphost` itself, not their own
submodule. There is no `import host` and no `mcphost_sdk` -- either fails
`host.tool_publish`/`host.spec_test` with `unknown_import` naming the real
modules. Call `host.quickstart kind=python` for the full list with one-line
signatures, plus a starter tool that publishes and persists through
`mcphost.table` on the first try.

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
