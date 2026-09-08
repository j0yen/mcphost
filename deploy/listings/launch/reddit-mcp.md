# Launch draft: r/mcp

Target: https://www.reddit.com/r/mcp/ (primary choice — on-topic
subreddit for MCP servers/hosts; r/LocalLLaMA is the fallback if r/mcp's
self-promotion rules block a host announcement at post time)

Headline (panel-tested winner, variant 0 — switch_share 0.314, wow_share
0.342 of 3 candidates, highest wow_share; see `panel-results/messaging.md`):

```
mcphost: your agent can host its own MCP tool — sign up, publish, call
```

Body:

```
mcphost is a streamable-HTTP MCP endpoint built for the case where the
agent itself is the one deciding it needs a new tool, mid-task, and
shouldn't have to hand that off to a human ops step to get it live.

The full path is MCP tool calls:

1. Connect with no credentials, call signup(name) -> tenant, bearer key,
   namespace, endpoint.
2. Reconnect authenticated. host.tool_publish(name, kind, spec) — wrap an
   API (`http`), ship code (`python`), or use `echo` to test the pipes.
   host.spec_test runs up to 5 example calls through the real sandbox
   before you commit to a tool row.
3. Call it: as <namespace>.<tool_name> in tools/list, or
   host.tool_call(name, args).
4. billing.plans() / billing.status() to see quota before you rely on
   volume.

Measured (panel run 0.26.3-20260908T085001Z, 21 sessions, method and raw
numbers in the repo): median signup -> first successful publish is 30.7s;
median signup -> first successful call on the agent's own tool is 42.4s.

Compares against: wiring FastMCP by hand (works, but a human stands up the
infra first) and general-purpose serverless (Vercel/Cloudflare Workers —
built for JS frontends, MCP wiring is manual).

Repo: https://github.com/j0yen/mcphost
Agent quickstart: https://github.com/j0yen/mcphost#quickstart-for-agents
```

Not posted — this is the prepared draft only (PRD non-goal: "Discord/Slack/HN
posts are a human act ... posting is Joe's decision"). If r/mcp's rules
block it, post to r/LocalLLaMA instead with the same body, first line
swapped to name the target sub.
