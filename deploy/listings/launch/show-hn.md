# Launch draft: Show HN

Target: https://news.ycombinator.com/submit (Show HN)

Headline (panel-tested winner, variant 0 — switch_share 0.314, wow_share
0.342 of 3 candidates, highest wow_share; see `panel-results/messaging.md`,
run `brief-gtm`, `synthorg gtm brief.md --test-variants variants.yaml
--only-test`):

```
Show HN: mcphost – your agent can host its own MCP tool: sign up, publish, call
```

URL: https://github.com/j0yen/mcphost

Body:

```
mcphost is a streamable-HTTP MCP endpoint where an authorized coding agent
signs up with one unauthenticated tool call and gets a live, callable tool
of its own — wrapping an API it already uses, or shipping a Python
function — without a separate deployment pipeline or a human in the loop.

The whole path is MCP tool calls, nothing else:

  signup(name) -> tenant, bearer key, namespace, endpoint
  host.tool_publish(name, kind, spec) -> live immediately, no restart
  host.tool_call(name, args) -> call it (or call it as <namespace>.<tool_name>)
  billing.plans() -> plan catalog, works anonymously

Measured (panel run 0.26.3-20260908T085001Z, 21 sessions): median time
from signup to a tenant's first successful host.tool_publish is 30.7s;
median time from signup to a successful call on that tenant's own tool is
42.4s. Numbers and the run they came from are in the repo:
docs/benchmarks/measure-0.26.3-20260908T085001Z.md.

Built because wiring FastMCP by hand, or bolting MCP onto a
general-purpose serverless platform, both put a human between "the agent
decided it needs a tool" and "the agent has the tool." mcphost collapses
that to zero.

Repo: https://github.com/j0yen/mcphost
Agent quickstart: https://github.com/j0yen/mcphost#quickstart-for-agents
llms.txt: https://mcphost.dev/llms.txt
```

Not posted — this is the prepared draft only (PRD non-goal: "Discord/Slack/HN
posts are a human act ... posting is Joe's decision"). Posting is tracked
by this PRD's open question ("Post the launch kit ... once listings are
live — you, or authorize the build to post under PUBLISH-OK?").
