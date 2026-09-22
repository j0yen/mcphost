# PRD: mcphost-uptime-probes — a schedule probes URLs and a `status` tool reads the results, no server

- Status: queued
- Lane: orch 2026-09-22T15:03:52.536911645+00:00 run=93
- build_target: shell
- build_into: /home/jsy/wintermute/mcphost
- build_priority: low
- publish: none
- Cited-tree: mcphost@f59b60b
- Vision: visions/mcphost-killer-apps.md
- Grounding: opportunity — visions/mcphost-killer-apps.md, component 4; schedule kind with 300 s floor (`src/triggers.rs`, `src/plans.rs`)
- PM: Joe
- Drafted: 2026-09-18
- Engineering target: mcphost.dev operations, `www/llms.txt` recipe section, `examples/uptime-probes/` proof, synthorg task; zero mcphost code changes

## TL;DR

A python tool `probe` fetches a list of URLs held in a state table `targets`, writes `{url, code, ms, at}` rows to `checks`, and a `host.trigger.set kind=schedule` runs it every 5 minutes (the free-plan floor). A `status` tool returns each target's last result and its up-percentage over 24 h. An agent that wants to know whether its own services are up gets StatusCake with no account and no box. Ships as an llms.txt section, a proof script, and a synthorg task.

## Problem statement

An agent builder running two or three small services (WHO) wants an agent to know when one is down (WHAT) and today either pays a monitoring SaaS or polls from a machine that is itself unmonitored (WHY). Consequence: the cheapest mcphost primitives, a schedule and a table, go unused, and the host's `host.trigger.*` surface has no worked example beyond synthetic tasks.

**Failure under this seed:** no — opportunity.

## Goals

- Under 2 minutes from signup to the first stored check on the free plan.
- `status` answers "is X up and since when" from rows, never from a live fetch.
- Bounded: at most 20 targets, 24 h retention enforced by the tool itself.

## Non-Goals

- Alerting (that is the webhook-inbox or agent-inbox recipe's job; the section links it).
- Intervals under 300 s; the vision's open question covers a pro-only floor.
- Any change to trigger or python-kind code.

## User stories

1. *Builder's agent* — I insert three URLs into `targets`, set the schedule, and 10 minutes later `status` shows two checks each.
2. *Builder's agent* — one service returns 503; `status` shows it down with the first failing timestamp.
3. *Joe* — the synthetic panel runs this and I see how many tenants get to a stored check.

## Requirements

**P0**
1. llms.txt section "Uptime probes with no server" (under 70 lines): `table_create targets`, `insert`, `tool_publish probe` (python, network on, 10 s per-URL timeout, 20-target cap), `trigger.set kind=schedule interval=300`, `tool_publish status`.
2. `probe` writes one `checks` row per target per run and deletes rows older than 24 h so the table stays under 20 targets × 288 runs.
3. `status` returns `[{url, last_code, last_ms, last_at, up_pct_24h, down_since?}]` from rows only.
4. `examples/uptime-probes/proof.sh`: fresh tenant, two targets (one healthy URL, one that returns 503), `host.trigger.fire` twice instead of waiting, assert two rows per target and `status` marks the 503 one down with `down_since`.
5. Signup tagged `source=recipe:uptime-probes`; free-plan fit stated: 2 tools, 2 tables, 1 schedule of 3, 288 calls/day of 500 at 20 targets or fewer.

**P1**
6. Synthorg task scored on reaching the first stored check.
7. Live test behind `MCPHOST_LIVE=1`.

**P2**
8. A one-line hand-off to the agent-inbox recipe: when `down_since` flips, `probe` sends an inbox message.

Non-functional: one `probe` run with 20 targets under 60 s (per-URL timeout 10 s, sequential is acceptable at P0).

## Success metrics

| metric | baseline | target | method | timeframe |
|---|---|---|---|---|
| primary: real tenants with a schedule and at least 10 stored checks | 0 | 5 | signup source tag + `checks` row count | 30 days |
| secondary: time to first stored check (synthetic, using `trigger.fire`) | none | p50 under 90 s | synthorg ledger | first 20 runs |
| guardrail: `checks` rows older than 24 h | n/a | 0 | proof assertion after a run | every Live run |

## Technical considerations

- The python tool reads and writes tables through the host pseudo-module (v0.56.2); network must be on for this tool (`sandbox.rs` supports `network: none`, so the publish must not request it).
- Calls/day accounting: each scheduled run is one call; the section shows the arithmetic so a free tenant does not exhaust the quota.
- `host.trigger.fire` lets the proof avoid the 300 s wait; the section still tells a real user to wait.

## Migration / compatibility

None.

## Open questions

| question | owner | due |
|---|---|---|
| Pro-only 60 s floor (vision open question) | Joe | at build |

## Acceptance criteria

1. P0 — Given llms.txt, When the section is read, Then it lists the two tables, two tools, and the schedule call, under 70 lines, with the calls/day arithmetic.
2. P0 — Given two targets and `host.trigger.fire` twice, When `checks` is queried, Then each target has exactly 2 rows with `code`, `ms`, `at`.
3. P0 — Given one target returning 503, When `status` is called, Then that target has `up_pct_24h` 0 and a `down_since` equal to its first check; the healthy target has 100 and no `down_since`.
4. P0 — Given a `checks` row older than 24 h inserted by the proof, When `probe` runs, Then the row is gone.
5. P0 — Given 21 targets, When `probe` runs, Then it probes 20 and reports the cap in its result.
6. P0 — Given the proof runs against https://mcphost.dev/mcp, When it finishes, Then AC2–5 hold and wall time is printed. (Live; evidence: proof stdout in the receipt)
7. P1 — Given the synthorg task, When run 5 times, Then 5 completions record time to first stored check.
8. P1 — Given `MCPHOST_LIVE` unset, When the suite runs, Then the Live test is skipped.
9. P2 — Given `down_since` flips for a target, When `probe` finishes, Then one inbox message exists for the owner.
