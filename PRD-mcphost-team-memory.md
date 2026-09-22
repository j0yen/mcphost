# PRD: mcphost-team-memory — one `remember`/`recall` pair a whole group of agents shares

- Status: queued
- Lane: orch 2026-09-22T02:44:48.424849184+00:00 run=65
- build_target: shell
- build_into: /home/jsy/wintermute/mcphost
- build_priority: normal
- publish: none
- Cited-tree: mcphost@f59b60b
- Vision: visions/mcphost-killer-apps.md
- Grounding: opportunity — visions/mcphost-killer-apps.md, component 5; group-shared tools run in the owner's tenant (`src/sharing.rs`), so a shared tool reaches the owner's tables
- PM: Joe
- Drafted: 2026-09-18
- Engineering target: mcphost.dev operations, `www/llms.txt` recipe section, `examples/team-memory/` proof, synthorg task; zero mcphost code changes

## TL;DR

An owner agent creates a table `memory`, publishes two python tools, `remember {key, text, tags?}` and `recall {query, limit?}`, and shares both to a group. Every agent in the group writes and reads the same memory; each row records which caller wrote it. The alternative is a vector database per agent or a shared document nobody's tool can search. Ships as an llms.txt section, a proof with three tenants, and a synthorg task. Search is substring and tag match at P0; ranking is a P2.

## Problem statement

A team running several agents (WHO) has no place those agents can all write to and search (WHAT) short of standing up a database or a vector store and wiring auth for each agent (WHY). Consequence: agents repeat work and contradict each other, and mcphost's group primitive (`host.group.*`, gating shared tools, `src/sharing.rs`) has no example beyond sharing a single wrapper.

**Failure under this seed:** no — opportunity.

## Goals

- Three tenants share one memory within 3 minutes on the free plan.
- Every row carries the writer's tenant id (from the call context the pseudo-module exposes) so a team can see who remembered what.
- Removing a member stops both tools for them on the next call.

## Non-Goals

- Semantic or embedding search (P2 lists ranking by recency and tag overlap only).
- Per-member quotas on the owner's table.
- Any change to sharing, state, or python-kind code.

## User stories

1. *Owner's agent* — I create the memory, publish and share `remember` and `recall`, add two teammates.
2. *Teammate's agent* — I `remember` "deploy window is Tuesday 9 pm"; the other teammate's `recall "deploy"` returns it with my id as writer.
3. *Owner* — I remove one member; their `recall` fails with a permission error, the other's still works.
4. *Joe* — the synthetic panel measures time to the first cross-agent recall.

## Requirements

**P0**
1. llms.txt section "Give your agents one memory" (under 70 lines): `table_create memory`, two `tool_publish` calls, `group.create`, two `tool_share visibility=group`, `group.add`.
2. `remember` inserts `{id, key, text, tags[], writer, at}` where `writer` is the calling tenant's id as exposed to the tool by the host; `recall` returns rows whose key, text, or tags contain the query, newest first, `limit` capped at 50.
3. `examples/team-memory/proof.sh`: three fresh tenants; owner sets up; tenant B remembers; tenant C recalls and the row shows B as writer; owner removes C; C's next recall is a permission error; wall time printed.
4. Signup of all three tagged `source=recipe:team-memory`; free-plan fit stated: 2 tools, 1 table, rows bounded by the 10k quota.

**P1**
5. Synthorg task: two synthetic agents in one group, scored on the first successful cross-agent recall.
6. Live test behind `MCPHOST_LIVE=1`.

**P2**
7. `recall` ranking by tag overlap then recency; `forget {id}` restricted to the writer or the owner.

Non-functional: `recall` p50 under 500 ms at 1,000 rows.

## Success metrics

| metric | baseline | target | method | timeframe |
|---|---|---|---|---|
| primary: groups with 2+ members and 10+ memory rows written by 2+ writers | 0 | 5 | state table scan on non-synthetic tenants | 30 days |
| secondary: time to first cross-agent recall (synthetic) | none | p50 under 120 s | synthorg ledger | first 20 runs |
| guardrail: a removed member reading memory | n/a | 0 | proof assertion | every Live run |

## Technical considerations

- Whether the pseudo-module exposes the caller's tenant id to a shared tool is the load-bearing assumption; the proof's first step checks it and the section says what `writer` holds. If it is absent, `writer` falls back to a caller-supplied `who` and the vision's open question gets a rust-extend follow-on.
- Shared tools execute in the owner's tenant, so the table is the owner's and counts against the owner's quotas; the section states this.
- Revocation is checked at call time (`src/sharing.rs`), so no republish is needed.

## Migration / compatibility

None.

## Open questions

| question | owner | due |
|---|---|---|
| Does the host expose the caller's tenant id inside a shared python tool today? If not, small rust-extend PRD | Joe / builder at step 1 | at build |

## Acceptance criteria

1. P0 — Given llms.txt, When the section is read, Then it lists the table, two tools, group creation, two shares, and the add call, under 70 lines.
2. P0 — Given three fresh tenants set up per the section, When tenant B calls `remember`, Then the row exists in the owner's `memory` table with `writer` = B's tenant id (or the documented fallback, stated in the proof output).
3. P0 — Given B's row, When tenant C calls `recall "deploy"`, Then the row is returned newest first with B as writer.
4. P0 — Given the owner removes C from the group, When C calls `recall`, Then a permission error is returned and B's `recall` still succeeds.
5. P0 — Given `recall` with `limit` 500, When called, Then at most 50 rows are returned.
6. P0 — Given the proof runs against https://mcphost.dev/mcp, When it finishes, Then AC2–5 hold and wall time is printed. (Live; evidence: proof stdout in the receipt)
7. P1 — Given the synthorg task, When run 5 times, Then 5 first-cross-agent-recall times are recorded.
8. P1 — Given `MCPHOST_LIVE` unset, When the suite runs, Then the Live test is skipped.
9. P2 — Given `forget {id}` by a tenant that is neither writer nor owner, When called, Then it is refused.
