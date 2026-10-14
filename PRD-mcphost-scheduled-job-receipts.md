# PRD — mcphost-scheduled-job-receipts: a scheduled job explains itself in one call, and three failures in a row reach the inbox

- Status: queued
- Lane: redbaron 2026-10-14T03:49:02.261993539+00:00 run=784
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- publish: j0yen/mcphost
- build_priority: high
- test_prefix: sjrc
- Grounding: creative (Joe 10-09 "take the leap")
- Falsifier: a test that derives `streak_failed`, `last_ok_unix` and `next_fire_unix` for a trigger from one `host.runs.list` call succeeds — then receipts add nothing and the PRD dies at AC 1
- Vision: visions/mcphost-killer-apps.md
- Loop: synthorg-chain: truth-tier satisfaction for workflow_orchestrator; recipes.by_recipe.mcphost-uptime-probes.satisfaction
- PM: Joe
- Drafted: 2026-10-09
- Engineering target: mcphost `src/triggers.rs` (482-528 `set_schedule`, 883 `spawn_scheduler`, `fire`, `pause`/`resume`), `src/runs.rs` (1124-1208 scheduled-run context, `trigger_ref` at 1203-1208), `src/handler.rs` (2412-2426 `host.runs.list` schema; `host.msg.send`), `src/hooks.rs` (event fires), `src/plans.rs:405-409`, `src/control.rs:53-100` `ceilings_json`, new migration `trigger_receipts`
- iter_log: 2026-10-13T21:52:43Z amended by /dream (priority from synthorg evidence, Joe 10-13 2:4x pm 'prioritize based on synthorg feedback') — build_priority normal→high: compete calibration 10-13: judges paid $15 (standup digest) and $25 (per-client cost report) for unattended scheduled-style tools, $0 for everything else

## TL;DR

After the session ends, a scheduled job runs unwatched. Today the only trace is `host.runs.list(trigger="schedule")` — runs, not a verdict: no failure streak, no next fire time, no excerpt, no alert. This PRD writes one receipt per fired trigger (status, duration, output excerpt, error code), exposes `host.schedule_receipts(trigger_id|tool)` with a per-trigger summary (`runs`, `ok`, `failed`, `streak_failed`, `last_ok_unix`, `next_fire_unix`), and sends one inbox message from `@host` when a trigger fails three times in a row, deduplicated until the next success.

## Problem statement

**Premise (agent's viewpoint):** an agent that set a schedule for its user will be asked "is it still running?" tomorrow; it wants one call that answers with a streak and the last output, and it wants to be told before being asked. The host records runs with `trigger_ref` (`src/runs.rs:1203-1208`) and fires schedules in `spawn_scheduler` (`src/triggers.rs:883`), but `host.runs.list` (`src/handler.rs:2412`) returns pages of runs with no per-trigger aggregate and no next-fire time, and nothing is pushed on failure.
**Falsifier:** if one `host.runs.list` call can yield `streak_failed` and `next_fire_unix` for a trigger, receipts are redundant. AC1 asserts the fields are absent from `runs.list` and present in `schedule_receipts`.
**Why not today:** PRD-mcphost-schedules (built 09-09) shipped fire + logs; PRD-mcphost-alerting-webhook (shipped 09-25) alerts on tool errors by webhook, not on a trigger's streak and not into the inbox.

## Goals / Non-goals

Goals: a receipt per fire (schedule, event, manual); one-call summary; a 3-streak inbox message with dedupe; retention per plan generated into quickstart limits; `host.trigger.get` carries the last receipt.
Non-goals: changing the schedule floor; retries (a failed fire is recorded, not re-run); replacing `host.runs.*`.

## User stories

1. As an agent (customer), the next day I call `host.schedule_receipts(tool="probe")` and tell my user "48 runs, 47 ok, last failure 03:10 timeout, next at 09:15".
2. As an agent waiting on `host.msg.wait`, a message `schedule probe failed 3× in a row` arrives with the last error, and I fix the tool before the user notices.
3. As a free-plan user, receipts cover 7 days; the number is in `host.quickstart.limits.plan`, not a doc.
4. As Joe, the uptime recipe's section can say "receipts" in one line instead of "grep tool_logs".

## Requirements

P0
- R1. Migration `trigger_receipts(tenant_id, trigger_id, run_id, source ∈ schedule|event|manual, started_unix, duration_ms, status, error_code?, output_excerpt ≤ 240 chars)`, written by the scheduler and by `host.trigger.fire`/event fires at run end; excerpts redacted like `host.tool_logs`.
- R2. `host.schedule_receipts(trigger_id?|tool?, limit ≤ 50, default 20)` returns `rows` newest first and `summary {runs, ok, failed, streak_failed, last_ok_unix, last_error, next_fire_unix}`; `next_fire_unix` from the scheduler's own computation.
- R3. When `streak_failed` reaches 3, one message from `@host` is sent to the tenant inbox: subject `schedule <tool> failed 3x in a row`, `data {trigger_id, tool, last_error, receipts_call}`; no further message until a success resets the streak; a paused trigger produces neither receipts nor messages.
- R4. Retention: free 7 days, pro 30 (`plans.rs` fields `receipts_retention_days`), purged by the existing sweeper; the field is rendered into `host.quickstart.limits.plan` through `ceilings_json`.
- R5. `host.trigger.get` carries `last_receipt` and `streak_failed`.

P1
- R6. `host.schedule_receipts` with `since_unix` filters rows; `limit` > 50 is `args_invalid`.

P2
- R7. A receipt row carries `cost_calls` so the summary can say calls consumed this week.

## Success metrics

| metric | baseline | target | method | when |
|---|---|---|---|---|
| calls to answer "is my schedule healthy" | ≥ 2 pages of `runs.list` + arithmetic | 1 | AC1 | at land |
| 3-streak failures with no inbox message | all | 0 | AC2 | at land |
| workflow_orchestrator satisfaction | 0.7x by segment (10-09 report) | +0.05 | nightly `measure.json` | 3 nightlies after land |
| receipt rows per fired run | 0 | 1.0 | count receipts vs runs with `trigger_ref` | first nightly |

## Technical considerations

- The scheduler already awaits the run's end to write `tool_logs` with `trigger_ref`; the receipt insert sits beside it, in the same transaction as the run's final status.
- Streak state is derived from the receipt table (last N rows) rather than a counter, so pause/resume and purges cannot desynchronize it.
- The inbox message reuses `host.msg.send` internals with sender `@host` (the same sender PRD-mcphost-agent-inbox reserved); `host.msg.wait` consumers receive it like any message.
- Receipts are per tenant and counted toward `state_bytes_max` (`src/plans.rs`) at ≤ 400 bytes per row; 288 fires/day × 7 days ≈ 800 KB on free.

## Migration / compatibility

One additive migration; existing triggers start with empty receipts and `streak_failed: 0`. `host.trigger.get` gains two fields (`Widened`). No change to `host.runs.*`.

## Open questions

| question | owner | due |
|---|---|---|
| Streak threshold 3 fixed, or a per-trigger `alert_after` parameter? Default: fixed 3, parameter later | Joe | at build |
| Should event-kind (webhook) triggers alert on 3 consecutive 401s (bad signer) the same way? Default: yes, same message | builder (recorded in PR) | at build |

## Acceptance criteria

1. P0 — Given a schedule fired 5 times (3 ok, then 2 failing) via `host.trigger.fire`, When `host.schedule_receipts(trigger_id)` is called once, Then it returns 5 rows and `summary.streak_failed == 2`, `summary.ok == 3`, `summary.next_fire_unix` set, while the same test asserts `host.runs.list(trigger="schedule")` carries none of `streak_failed`, `last_ok_unix`, `next_fire_unix` (premise holds; if it does, the test fails).
2. P0 — Given the third consecutive failure, When the run ends, Then exactly one message from `@host` with subject `schedule <tool> failed 3x in a row` and `data.trigger_id` is in `host.msg.inbox`; the fourth failure sends nothing; a success then two more failures send nothing; a success then three failures send one more.
3. P0 — Given a run whose result is 1 000 chars and contains a value matching `(?i)secret`, When the receipt is written, Then `output_excerpt` is 240 chars plus `…`, the secret value is `•••`, and `run_id` resolves through `host.runs.part`.
4. P0 — Given `host.trigger.pause`, When the scheduler ticks three times, Then no receipt and no message is produced; Given `host.trigger.resume`, Then the next fire's receipt exists and `streak_failed` restarts at 0.
5. P0 — Given a free tenant with receipts 8 days old, When the sweeper runs, Then those rows are gone, `summary.runs` counts only the retained ones, and keyed `host.quickstart.limits.plan.receipts_retention_days == 7` (pro: 30).
6. P0 — Given a manual `host.trigger.fire`, a scheduled fire, and a signed POST to an event trigger, When each run ends, Then three receipts exist with `source` manual, schedule and event respectively, and `host.trigger.get` on each shows `last_receipt`.
7. P1 — Given `limit: 51`, When called, Then `args_invalid` names `limit ≤ 50`; Given a tool with no triggers, Then `rows` is empty and `summary.runs == 0` without error.
8. P0 — Given prod after this lands, When the proof script signs up, publishes a tool, sets a schedule, fires it three times and lists receipts, Then it prints `receipts=3 streak_failed=0` and exits 0 (Live; evidence: `bash examples/scheduled-job-receipts/proof.sh https://mcphost.dev/mcp`)
