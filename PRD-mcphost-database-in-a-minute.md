# PRD: mcphost-database-in-a-minute — a CSV becomes a Claude-queryable table in one minute

- Status: queued
- Lane: orch 2026-09-22T15:03:43.163908385+00:00 run=91
- build_target: shell
- build_into: /home/jsy/wintermute/mcphost
- build_priority: normal
- publish: none
- Cited-tree: mcphost@f59b60b
- Vision: visions/mcphost-killer-apps.md
- Grounding: opportunity — visions/mcphost-killer-apps.md, leaf 2 (completes on free-plan quotas)
- PM: Joe
- Drafted: 2026-09-18
- Engineering target: mcphost.dev operations, `www/llms.txt` recipe section, `examples/database-in-a-minute/` proof script, synthorg mcphost consumer corpus; zero mcphost code changes

## TL;DR

An agent creates a table with `host.state.table_create`, loads a CSV with `host.state.insert`, publishes a `python` tool `query` that reads the table through the host pseudo-module (mcphost-stdlib-pseudo-modules, v0.56.2), and the user connects Claude Desktop to the tenant through the registry listing. From "I have a CSV" to "Claude answered a question about it" in one minute, no server, no Airtable account. Ships as an llms.txt section, a proof script with a 1,000-row fixture, and a synthorg task.

## Problem statement

A person with a spreadsheet and a Claude Pro subscription (WHO) wants Claude to answer questions over their own rows (WHAT) and today must either paste the CSV into every conversation or set up Airtable or Notion plus its MCP server (WHY). Consequence: the most common personal-data job stays manual, and mcphost's state tables (SQL-backed, WHERE filters, `rows_per_table_max` 10k and `state_bytes_max` 100 MB on free, `src/state.rs`, `src/plans.rs`) go unused by anyone but synthetic tenants.

**Failure under this seed:** no — opportunity.

## Goals

- One section, one proof: CSV in, question answered, under 60 s on the free plan.
- The `query` tool exposes a safe subset: column filter, equality/range WHERE, limit, no raw SQL from the caller.
- Counted by recipe tag.

## Non-Goals

- Uploads larger than the free row quota (10k rows); the section says so.
- Joins across tables, aggregation beyond count/sum/avg on one column.
- Any change to state or python-kind code.

## User stories

1. *Owner's agent* — I create `expenses`, insert 1,000 rows in batches, publish `query`, and answer "total spend in March" from Claude Desktop.
2. *Owner* — I add a row from Claude; the next query sees it.
3. *Joe* — the synthetic panel runs this task and I see p50 time to first answered question.

## Requirements

**P0**
1. llms.txt section "Give Claude a database in one minute" (under 70 lines): `host.state.table_create`, batched `host.state.insert` (batch size stated), `host.tool_publish` kind python for `query` with the pseudo-module call, and the Claude Desktop connection line via the registry.
2. `examples/database-in-a-minute/proof.sh` with a 1,000-row CSV fixture: fresh tenant, table, batched insert, publish `query`, three questions (equality, range, count) asserted against known answers, wall time printed.
3. The `query` tool accepts `{columns?, where?: [{col, op, value}], limit?}` with `op` in `= != < > <= >=`, rejects anything else, and caps `limit` at 500.
4. Signup tagged `source=recipe:database-in-a-minute`.
5. Free-plan fit stated in the section: 1 tool, 1 table, 1,000 rows of the 10k row quota, batches sized to stay under `state_bytes_max`.

**P1**
6. Synthorg task: load fixture, ask the three questions, score exact answers.
7. Live test in the suite behind `MCPHOST_LIVE=1`.

**P2**
8. A `csv_import` helper tool (python kind) that takes the CSV text and does the batching itself.

Non-functional: 1,000-row import under 30 s against prod; query p50 under 500 ms.

## Success metrics

| metric | baseline | target | method | timeframe |
|---|---|---|---|---|
| primary: real tenants with a table and a `query` tool created within 10 min of signup | 0 | 10 | signup_events source tag + state table count | 30 days |
| secondary: import + first answered question wall time (synthetic) | none | p50 under 60 s | synthorg ledger | first 20 runs |
| guardrail: `query` accepting raw SQL | n/a | 0 | proof negative test | every Live run |

## Technical considerations

- The pseudo-module is the only supported path from a python tool to the owner's table; the section must show its exact import line from v0.56.2's docs.
- `rows_per_table_max` and `state_bytes_max` come from `src/plans.rs`; the proof reads them from `host.usage` rather than hardcoding.
- Claude Desktop connects through the registry entry `dev.mcphost/mcphost` with the tenant's bearer key; the section links the existing connection doc rather than repeating it.

## Migration / compatibility

None.

## Open questions

| question | owner | due |
|---|---|---|
| Whether `csv_import` (P2) should be a host-provided template tool instead of per-tenant python | Joe | at build |

## Acceptance criteria

1. P0 — Given llms.txt, When the section is read, Then it shows table_create, batched insert, the `query` publish, and the Desktop connection line, under 70 lines.
2. P0 — Given the 1,000-row fixture and a fresh tenant, When `proof.sh` runs, Then all rows are present (`host.state.query` count = 1000).
3. P0 — Given the published `query` tool, When asked the equality, range, and count questions, Then each answer matches the fixture's known value.
4. P0 — Given a `where` with `op` = `LIKE` or a raw SQL string, When `query` is called, Then it returns a validation error and no rows.
5. P0 — Given the proof completed, When `signup_events` is read, Then `source=recipe:database-in-a-minute`.
6. P0 — Given the proof runs against https://mcphost.dev/mcp, When it finishes, Then import is under 30 s and all assertions pass. (Live; evidence: proof stdout in the receipt)
7. P1 — Given the synthorg task, When run 5 times, Then 5 exact-answer completions are recorded.
8. P1 — Given `MCPHOST_LIVE` unset, When the suite runs, Then the Live test is skipped.
9. P2 — Given `csv_import`, When given the fixture text, Then the table holds 1,000 rows without the caller batching.
