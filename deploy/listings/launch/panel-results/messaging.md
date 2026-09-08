# Messaging variants

tie_break: ties go to the earlier line in the variants file

## Variant 0 (winner)

Your agent can host its own MCP tool: sign up, publish, call — median 30.7s to first publish, 42.4s to first call, measured.

- switch_share: 0.314
- wow_share: 0.342

## Variant 1

mcphost: an MCP endpoint where signup is one tool call and the tool your agent just published is live immediately — no restart, no review queue.

- switch_share: 0.308
- wow_share: 0.338

## Variant 2

Ship an MCP tool from inside your agent's own task, not a separate deployment project — no infra to stand up first.

- switch_share: 0.311
- wow_share: 0.339

