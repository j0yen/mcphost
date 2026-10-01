# Form draft: Smithery

Submission surface: https://smithery.ai (server submission form / GitHub
connection flow as of the site's current onboarding -- confirm at
submission time, this is a directory whose intake mechanism changes).

## Payload

- Repository: https://github.com/j0yen/mcphost
- Name: mcphost
- Version: 0.64.0
- Endpoint: https://mcphost.dev/mcp (streamable-HTTP, no auth required to
  connect -- the only tool offered before a bearer key is `signup`)
- Homepage: https://mcphost.dev
- Category: Developer Tools / Agent Infrastructure
- Tags: agent-backend, mcp-server, hosting, webhooks, scheduling, database,
  tool-publishing

### One-line description

Hosted MCP runtime where the agent is the operator: sign up by one tool
call, then publish, call, and manage your own tools immediately -- no
local install, no restart.

### Three-line description

mcphost gives an agent a persistent backend it provisions for itself in
one call: sign up with no credentials, publish a tool, and call it
immediately -- no restart, no deploy, no human in the loop.
Built for agent operators whose coding agents need to keep working
unattended: share a tool without sharing the key, turn a CSV into a
queryable database, receive webhooks as an agent, run uptime probes with
no server, or give a team of agents one shared memory.
Free to start -- 500 calls/day, no card required; Pro is $19/month for
50,000 calls included, then $0.001/call.

### Install / connect

```
claude mcp add --transport http mcphost https://mcphost.dev/mcp
```
(Codex CLI: `codex mcp add mcphost --url https://mcphost.dev/mcp`; Cursor
and Claude.ai connect over the same URL -- see README.md "Connect" for
every client's exact snippet.)

### Working example (verified against src/handler.rs's tool registry)

```
signup(name="my-agent")
# -> {tenant, key, namespace, endpoint}
# reconnect with Authorization: Bearer <key>
host.tool_publish(name="hello", kind="python",
                   spec={"source": "def main(args):\n    return {'ok': True}"})
host.tool_call(name="hello", args={})
```
This is docs/agent-quickstart.md steps 2 and 4-5; `signup`, `host.tool_publish`,
and `host.tool_call` are all present in `src/handler.rs`'s Tool::new registry
(grepped 2026-09-30, chore/listings-wave1).

Not yet executed -- this file is the prepared artifact only. Nothing is
submitted without `PUBLISH-OK` (see `scripts/listings-submit.sh`).
