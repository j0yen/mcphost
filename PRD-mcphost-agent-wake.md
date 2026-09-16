# PRD — agent wake: a message arrival fires a bound tool, and a client can long-poll

- Status: queued
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_priority: high
- build_version_bump: minor
- test_prefix: wake
- publish: j0yen/private
- deferred_acs: [9, 10]
- deferred_ac_reasons: {"9": "P1 perf upgrade (tokio Notify per-tenant wake instead of the 500ms poll host.msg.wait ships with); the P0 poll already satisfies the Goals-section p95<5s target and the doc comment on messaging::wait names this deferral explicitly, mirroring host.runs.wait's own P1 notify deferral in this same crate.", "10": "P1 thread_id scoping filter on host.trigger.set(kind=\"message\"); the from-address filter (P0 requirement 1, AC2) already ships and covers the Goals-section scope, thread_id narrowing is an additive filter with no P0 dependency."}
- Vision: visions/mcphost-agent-messaging.md
- Depends-on: PRD-mcphost-agent-inbox.md
- Loop: mcphost-buildloop: coordinate_rate
- PM: Joe
- Drafted: 2026-09-13
- Engineering target: extend ~/wintermute/mcphost: a third trigger `kind = 'message'` in `src/triggers.rs` beside `schedule` and `event`, firing through `runs::enqueue` (`src/runs.rs:285`) with the message envelope as the tool argument and `event_dedupe` keyed on the message id; `host.msg.wait` long-poll modelled on `host.runs.wait` (`src/handler.rs:840`, 25 s cap)

## TL;DR

An inbox that only a returning agent reads is a ledger. This PRD lets a tenant bind one of its tools to fire whenever a message arrives — the tool runs with the message as its argument, in the same `runs` ledger, with the same dedupe, quotas and `host.runs.*` inspection as an inbound webhook — so the agent's work proceeds even when its session is gone. For a client that is present but has no polling loop, `host.msg.wait` blocks up to 25 seconds for the next message. This is the fix for the vision's weakest link: recipients that never come back.

## Problem statement

mcphost is stateless per request and cannot wake an agent's session. What it can wake is a tool: `triggers` rows with `kind ∈ {schedule, event}` (`migrations/0015_triggers.sql`, `UNIQUE(tenant_id, tool_name, kind, config_hash)`) already enqueue runs when a cron fires or a signed webhook lands (`src/hooks.rs:673`: verify, dedupe, enqueue), and `host.trigger.*` gives the tenant list/pause/resume/replay/test over them. A message arrival is a third source of the same shape and today reaches nothing.

For a present client, `host.runs.wait` shows the host already supports a bounded long-poll ("max 25s, returning its current status either way — for a client with no polling loop of its own", `handler.rs:840`). An agent waiting for a reply has no equivalent and must call `host.msg.inbox` in a loop, spending its `calls_per_day` quota on empty pages.

wintermute-reach's lesson applies: every delivery produces an ack event and a failed transport acks `delivered: false` rather than dropping silently. Here the run row is the ack.

Pain, read back to the agent: "Someone answered you an hour after your session ended. Nothing happened."

## Goals

- A message to a tenant with a `message` trigger produces exactly one run of the bound tool, with the message envelope as its argument, within 5 s of the send.
- Message-fired runs are indistinguishable in the ledger from event-fired ones: same `host.runs.get/list/wait`, same quotas, same replay.
- `host.msg.wait` returns the next inbox page within 25 s or an empty page at the deadline, never an error.

## Non-goals

- Waking an external process (no webhook out, no push). Filtering triggers by sender or thread beyond a single optional `from` filter (P1). Delivery guarantees stronger than at-least-once with dedupe. Channels (PRD-mcphost-agent-channels) — channel posts fire triggers only in that PRD.

## User stories

- **Agent, absent.** As an agent that publishes a `python` tool `handle_msg` and binds it with `host.trigger.set(tool="handle_msg", kind="message")`, I want a message from `@planner` to run `handle_msg` with the message as `args` so that my reply logic runs while I am gone.
- **Agent, present.** As an agent that just asked a question, I want `host.msg.wait(cursor, timeout_s=25)` so that I get the reply the moment it lands instead of polling.
- **Agent, debugging.** As an agent whose bound tool failed, I want the run in `host.runs.list(trigger="message")` with `error_class` and the message id so that I can `host.trigger.replay(run_id)`.
- **Operator.** As the operator, I want message-fired runs counted against the same `jobs_concurrent` and per-day quotas as everything else so that a message storm cannot outrun the plan.

## Requirements

**P0**
1. `host.trigger.set(tool, kind="message", from?: address)` creates a `triggers` row with `kind='message'`, `config_json={"from": …}`; one message trigger per `(tenant, tool, from)`; the tenant's `event_triggers_max` quota counts message triggers.
2. On every stored delivery to recipient R (inbox PRD requirement 2 and 3), for each enabled `message` trigger of R whose `from` is null or equals the sender's address, enqueue one run through `runs::enqueue(state, R, tool, envelope)` where `envelope = {message_id, thread_id, seq, from, body, data, in_reply_to, created_at}`; the run row has `trigger='message'` and stores `message_id`.
3. Dedupe: `event_dedupe` (or an equivalent row) keyed `(trigger_id, message_id)`; a second delivery attempt of the same message returns the cached `run_id` and enqueues nothing.
4. The enqueue happens after the message commit, never inside its transaction; an enqueue failure (quota, sandbox down) writes a run row with `status='rejected'` and `error_class` so the failure is visible in `host.runs.list` — the message itself is still stored.
5. `host.trigger.pause/resume/remove/list/get/test/replay` work on message triggers; `test` fires the tool with a synthetic envelope marked `test: true`.
6. `host.msg.wait(cursor?, timeout_s ≤ 25, unread_only?)`: returns the same shape as `host.msg.inbox` as soon as a matching message exists, else at the deadline with an empty list and the unchanged cursor; concurrency-limited by the existing `concurrent_calls_per_tenant`.
7. Message-fired runs honour `jobs_concurrent` and `calls_per_day` exactly as event-fired ones.

**P1**
8. `host.trigger.set(kind="message", thread_id?)` filter, so an agent can bind a tool to one conversation.
9. `host.msg.wait` uses an in-process notify (tokio `Notify` per tenant) rather than sleeping and re-querying, so a delivery wakes waiters within 100 ms.

**P2**
10. A bound tool's structured result with `{"reply": "..."}` is posted back into the thread as R's reply, so a trivial responder needs no `host.msg.reply` call of its own (off by default; `auto_reply: true` on the trigger).

## Success metrics

| metric | baseline | target | method | timeframe |
|---|---|---|---|---|
| Time from send to bound-tool run start | n/a (no path) | p95 < 5 s | AC 1 | at ship |
| Duplicate runs per message | n/a | 0 | AC 3 | at ship |
| `host.msg.wait` wake latency after delivery | n/a | p95 < 1 s (P0), < 100 ms (P1) | AC 6, AC 9 | at ship |
| `coordinate_rate` with a wake-bound responder in fake mode | 0 | ≥ 0.9 | harness paired task, responder variant | first measure after ship |

## Technical considerations

- Reuse `hooks.rs`'s verify-then-enqueue tail (`handle_hook`, `replay`) by factoring the enqueue-with-dedupe step into a function both hooks and messaging call; do not copy it.
- The scheduler tick (`triggers::spawn_scheduler`) is not involved: message triggers fire on the send path. Keep the send path's added latency under 20 ms by doing trigger lookup with an index on `(tenant_id, kind, enabled)`.
- `host.msg.wait`'s P0 may poll SQLite every 500 ms up to the deadline; P1 replaces the poll with a notify.
- The envelope must never include the recipient's block list, receipts, or other participants' addresses beyond what `host.msg.thread` would show that recipient.

## Migration / compatibility

Additive: a new `kind` value, one index, no shape change to existing triggers or runs. Existing `event_triggers_max` quota semantics widen to include message triggers; the plan defaults are unchanged.

## Open questions

| question | owner | due |
|---|---|---|
| Should `auto_reply` (P2) ship at all, or is it a footgun for loops between two auto-replying agents (drafted: off by default, and a run fired by an auto-reply never itself auto-replies) | Joe | at build |

## Acceptance criteria

1. P0 — Given tenant R published tool `handle_msg` and called `host.trigger.set(tool="handle_msg", kind="message")`, When tenant S sends R a message, Then within 5 s `host.runs.list(trigger="message")` for R shows one run whose args equal the message envelope (`message_id`, `thread_id`, `from` = S's address, `body`).
2. P0 — Given the trigger from AC 1 with `from` set to tenant T's address, When S sends R a message, Then no run is enqueued; and When T sends, Then one run is.
3. P0 — Given the trigger from AC 1, When the same message is delivered twice through the internal delivery path (simulated retry), Then exactly one run exists for its `message_id` and the second attempt returns the first `run_id`.
4. P0 — Given R's plan has `jobs_concurrent = 1` and one run is in flight, When S sends R a second message, Then the message is stored and readable in R's inbox, and a run row exists with `status="rejected"` and a non-empty `error_class`.
5. P0 — Given a paused message trigger, When S sends R a message, Then no run is enqueued; and after `host.trigger.resume`, When S sends again, Then one run is.
6. P0 — Given R has called `host.msg.wait(cursor=c, timeout_s=25)`, When S sends R a message 2 s later, Then the call returns within 3 s of the send with that message and an advanced cursor; and When no message arrives, Then the call returns at 25 s ± 1 s with an empty list and cursor `c`.
7. P0 — Given `host.trigger.test(trigger_id)` on a message trigger, When it runs, Then the tool receives an envelope with `test: true` and no `messages` row is created.
8. P0 — Given a message-fired run failed, When R calls `host.trigger.replay(run_id)`, Then a new run executes with the identical envelope and the original run is unchanged.
9. P1 — Given 50 waiters on `host.msg.wait` for 50 tenants, When each receives one message, Then p95 time from message commit to wait return is under 100 ms.
10. P1 — Given a trigger with `thread_id` set, When a message lands in a different thread, Then no run fires; and in that thread, Then one fires.
