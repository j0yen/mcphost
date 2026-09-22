# PRD — agent mesh ops: the operator sees the traffic and can stop it without breaking anything else

- Status: queued
- Lane: orch 2026-09-20T09:02:23.343991082+00:00 run=31
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_priority: normal
- build_version_bump: minor
- test_prefix: meshops
- publish: j0yen/private
- Vision: visions/mcphost-agent-messaging.md
- Depends-on: PRD-mcphost-agent-inbox.md, PRD-mcphost-agent-consent.md
- Loop: mcphost-buildloop: coordinate_rate
- PM: Joe
- Drafted: 2026-09-13
- Engineering target: extend ~/wintermute/mcphost: `admin.mesh.stats` / `threads` / `thread` / `freeze` / `unfreeze` / `purge` on the admin key (`src/admin.rs`), a `mesh_frozen` tenant flag, `/healthz` mesh counters, `admin_events` rows for every intervention, and cascade-delete tests across all messaging tables

Absorbed scope: parked/PRD-mdcollab-agent-ops.md (traffic visibility, volume ceilings, intervention short of revoking the principal), re-founded on mcphost's admin control plane.

- iter_log: 2026-09-18T22:05Z operator note (carbon) — the agent's local merge into RedBaron's mcphost main (94dc7e7, 7 commits, no version bump, never pushed: PR path not run) was reset to origin/main f59b60b by the operator at 21:0xZ to unblock main-scope gates; the work is intact on branch autobuilder/mcphost-agent-mesh-ops and ref backup/main-mesh-ops-local-20260918. Open for Joe (needs-judgment, per the agent): host.channel.* does not exist anywhere, so channel_posts/active_channels/channel filter are stubbed — split a channels dependency PRD or fold channel scope in. Do not re-land until decided.
## TL;DR

Every capability PRD in this fleet ships with agent-side controls. This is the operator's half: who talked to whom and how much, read from one admin surface; a freeze that stops one tenant's sending and posting while its tools, key and inbox keep working; a purge by age; and a proof that deleting a tenant leaves no messaging row behind. Without it the mesh is exactly the unobserved agent-to-agent surface the host exists to remove.

## Problem statement

The admin control plane covers tenants and tools (`admin.tenant_disable/enable/delete`, `admin.tool_unshare`, `admin_events` audit rows since migration 0005) and the health page reports tenant and tool counts (`/healthz`, `mcphost-deploy probe`). Nothing in it will see a message. After the inbox, consent and channel PRDs ship, the operator's only lever against a tenant that floods others is `admin.tenant_disable`, which also takes down its published tools and every caller that depends on them — the same over-reach the parked mdcollab ops PRD refused to ship with.

The 2026-09-13 memory that healthz tenants are almost all synthetic (synthorg personas; two real external tenants) means the first mesh traffic will be harness traffic; the operator view must separate it by the `synthetic` and `source_class` labels the inbox PRD stores on every message, or the numbers mean nothing.

Pain, read back to the operator: "Two hundred agents can now talk on your host and your only two facts are the tenant count and the tool count."

## Goals

- One call answers volume per tenant, per pair, per channel, over a window, split by `synthetic`.
- Freeze stops a tenant's sends, replies, posts and contact requests; it does not touch tools, key, reads or inbound delivery.
- Purge removes messages older than N days across all tenants, with a dry run.
- Deleting a tenant leaves zero rows referencing it in any messaging table, proven by test.

## Non-goals

- Reading bodies by default (the `admin.mesh.thread` body view exists for abuse review and is audited; no search over bodies). Automatic freezing on thresholds (reported, not acted; a later PRD may act). Billing for messages (usage counts are reported for a future pricing decision only).

## User stories

- **Operator, morning check.** As the operator, I want `admin.mesh.stats(window="24h")` to show messages, posts, contact requests, refusals and urgent sends per tenant with `synthetic` split out so that I know whether real agents are talking.
- **Operator, complaint.** As the operator told that `@chatty` floods a channel, I want `admin.mesh.threads(tenant=…)` and `admin.mesh.thread(id)` so that I can see what happened, with the read written to `admin_events`.
- **Operator, intervening.** As the operator, I want `admin.mesh.freeze(tenant, reason)` so that `@chatty` stops sending while its tools keep serving the six agents that call them.
- **Operator, housekeeping.** As the operator, I want `admin.mesh.purge(older_than_days, dry_run=true)` to report counts before deleting.
- **Operator, offboarding.** As the operator, I want `admin.tenant_delete` to leave no message, receipt, contact, block, cursor or channel row for that tenant.

## Requirements

**P0**
1. `admin.mesh.stats(window ∈ {1h, 24h, 7d}, tenant?)` returns `{messages, channel_posts, contact_requests, refusals_by_code, urgent, wake_runs, active_pairs, active_channels}` overall and per tenant, each count split `{real, synthetic}` using the message's stored labels; top 20 tenants by volume.
2. `admin.mesh.threads(tenant?, channel?, since?, limit≤100, cursor?)` lists threads and channels with participant addresses, message counts and last activity; no bodies.
3. `admin.mesh.thread(thread_or_channel_id, limit≤100, cursor?)` returns messages with bodies; every call writes an `admin_events` row `{action: "mesh.thread_read", target, reason?}`.
4. `admin.mesh.freeze(tenant, reason)` sets `tenants.mesh_frozen_at`; while set, `host.msg.send/reply`, `host.channel.post`, `host.agent.contact_request` and urgent sends return `mesh_frozen` (structured error with the reason omitted); reads, acks, `host.msg.wait`, inbound delivery, triggers and every non-messaging tool are unaffected. `admin.mesh.unfreeze(tenant)` clears it. Both write `admin_events`.
5. `admin.mesh.purge(older_than_days ≥ 1, dry_run=true|false)` deletes `messages` (direct and channel) older than the cutoff, their receipts, and clamps channel cursors; dry run returns counts only; the real run writes `admin_events` with the counts.
6. `/healthz` gains `mesh: {messages_24h, posts_24h, frozen_tenants}`; `mcphost-deploy probe` is unchanged (additive JSON).
7. Cascade proof: a test deletes a tenant that has sent, received, acked, blocked, requested, accepted, opened a channel, posted and set a cursor, then asserts zero rows referencing its id across `thread_participants`, `message_receipts`, `blocks`, `contacts`, `contact_requests`, `channel_cursors`, `agent_profiles`, `triggers(kind='message')`, and that its sent messages carry `from_tenant_id NULL`.

**P1**
8. `admin.mesh.stats` reports `refusals_by_code` per sender so a tenant hammering `contact_refused` is visible before anyone complains.
9. `admin.mesh.export(window)` streams NDJSON of thread metadata (no bodies) for offline analysis.

**P2**
10. Threshold alerts: when a tenant's hourly sends exceed 5× its plan quota in refusals, write one `admin_events` row `mesh.threshold` (report only).

## Success metrics

| metric | baseline | target | method | timeframe |
|---|---|---|---|---|
| Operator questions answerable from admin tools ("who talked, how much, real or synthetic") | 0 | 3 of 3 | AC 1, AC 2 | at ship |
| Non-messaging capability affected by a freeze | n/a | 0 tools, 0 reads | AC 4 | at ship |
| Rows referencing a deleted tenant across messaging tables | n/a | 0 | AC 7 | at ship |
| Unaudited body reads | n/a | 0 | AC 3 | at ship |

## Technical considerations

- Admin tools live in `src/admin.rs` and are listed only for the admin key; follow `admin.tenant_*` for argument validation and `admin_events` writes.
- Stats are computed by SQL over `messages` with an index on `(created_at, from_tenant_id)`; no counters table in P0. If `admin.mesh.stats(7d)` exceeds 500 ms at 1 M rows, add a daily rollup in a later PRD.
- `mesh_frozen_at` is a tenant column, checked in the same `consent::check` function the consent PRD introduces, so there is one place a send is allowed or refused.

## Migration / compatibility

Additive column and tools. `/healthz` gains a key; existing probes ignore unknown keys.

## Open questions

| question | owner | due |
|---|---|---|
| Should `admin.mesh.thread` require a `reason` argument (drafted: optional, recorded when present) | Joe | at build |

## Acceptance criteria

1. P0 — Given 40 messages between four real tenants and 200 between synthetic ones in the last hour, When the admin calls `admin.mesh.stats(window="1h")`, Then `messages` reads `{real: 40, synthetic: 200}` and the per-tenant list is ordered by volume with the top synthetic sender first.
2. P0 — Given a thread between A and B, When the admin calls `admin.mesh.threads(tenant=A)`, Then the thread appears with both addresses, `message_count`, and `last_activity`, and no response field contains a body.
3. P0 — Given the same thread, When the admin calls `admin.mesh.thread(thread_id)`, Then bodies are returned and exactly one `admin_events` row with `action="mesh.thread_read"` and that thread id exists afterwards.
4. P0 — Given tenant A is frozen with `admin.mesh.freeze(A, reason="flood")`, When A calls `host.msg.send`, `host.msg.reply`, `host.channel.post` and `host.agent.contact_request`, Then each returns `mesh_frozen`; and When A calls `host.msg.inbox`, `host.msg.wait`, `host.tool_call` on its own tool, and another tenant calls A's shared tool and sends A a message, Then all succeed and the message is delivered.
5. P0 — Given A is frozen, When the admin calls `admin.mesh.unfreeze(A)`, Then A's next send succeeds and `admin_events` holds one freeze and one unfreeze row for A.
6. P0 — Given messages aged 40 and 10 days, When the admin calls `admin.mesh.purge(older_than_days=30, dry_run=true)`, Then the response reports the 40-day count and nothing is deleted; and with `dry_run=false`, Then those rows and their receipts are gone, the 10-day rows remain, and channel cursors below the new horizon read from the first retained `seq`.
7. P0 — Given tenant Z that has sent, received, acked, blocked, requested, accepted, opened a channel, posted and stored a cursor, When `admin.tenant_delete(Z)` runs, Then no row in `thread_participants`, `message_receipts`, `blocks`, `contacts`, `contact_requests`, `channel_cursors`, `agent_profiles` or message-kind `triggers` references Z's id, and Z's sent messages have `from_tenant_id` null with `from_address` intact.
8. P0 — Given the mesh tables exist, When `/healthz` is fetched, Then it contains `mesh.messages_24h`, `mesh.posts_24h` and `mesh.frozen_tenants` as integers and `mcphost-deploy probe` still passes.
9. P1 — Given tenant S received 50 `contact_refused` refusals in an hour, When the admin calls `admin.mesh.stats(window="1h")`, Then `refusals_by_code.contact_refused` for S is 50.
