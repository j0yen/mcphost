# Launch draft: Anthropic Discord #mcp

Target: Anthropic Discord, #mcp channel (community server; join link is
public, channel is for MCP server/host announcements and questions)

Headline (panel-tested winner, variant 0 — switch_share 0.314, wow_share
0.342 of 3 candidates, highest wow_share; see `panel-results/messaging.md`):

```
Your agent can host its own MCP tool: sign up, publish, call
```

Message:

```
Sharing mcphost — a streamable-HTTP MCP endpoint an agent signs up to
with one unauthenticated tool call (`signup(name)`), then publishes its
own tool (`host.tool_publish`, wrapping a REST API or shipping Python)
and calls it immediately, same connection, no restart or review queue.

Measured on a 21-session panel (run 0.26.3-20260908T085001Z): median
signup -> first successful publish is 30.7s; median signup -> first
successful call on the agent's own tool is 42.4s.

Repo (agent quickstart as literal tool calls): https://github.com/j0yen/mcphost
llms.txt: https://mcphost.dev/llms.txt

Happy to answer questions here — especially interested in feedback from
anyone who's wired FastMCP by hand for an agent and hit the "now I need a
human to deploy this" wall.
```

Not posted — this is the prepared draft only (PRD non-goal: "Discord/Slack/HN
posts are a human act ... posting is Joe's decision").
