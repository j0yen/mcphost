# PRD: mcphost-data-retention — the database grows by policy, not by disk

- Status: queued
- Lane: orch 2026-09-20T06:20:52.942126986+00:00 run=24
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_priority: normal
- publish: j0yen/private
- Vision: visions/mcp-host.md
- Loop: mcphost-buildloop: satisfaction — timeliness under a bounded database
- Grounding: wwhtbt — visions/mcp-host.md addendum 2026-09-18, leaf 2: eighteen migrations add per-call and per-event tables; the only purge is tenant-driven `host.runs.purge` (`handler.rs:824`); the 09-12 decision set 30-day access-log retention and the database has none
- PM: Joe
- Drafted: 2026-09-18
- Engineering target: mcphost `src/db.rs`, new `src/retention.rs`, `src/admin.rs` (`admin.usage` size fields), a migration adding `retention_policy`

## TL;DR

Every call, meter tick, signup, and event is a row that stays forever. On a ccx13 with
one disk shared with the site and the hub, that is a slow outage with a known date. This
PRD adds a per-table retention policy with billing-safe defaults, a nightly prune inside
the process, database size and per-table row counts in `admin.usage`, and a disk guard
that refuses writes before the box fills.

## Problem statement

Joe cannot say how large the database is or when the disk fills. Evidence: migrations
`0004_calls_resource_usage`, `0007_metering`, `0009_call_outcome`, `0016_event_dedupe`
create append-only tables; `grep -rniE 'retention|prune|vacuum|purge' src migrations`
finds only `host.runs.purge` and the deploy-time `VACUUM INTO`; a 25,000-call stress
run (PRD-mcphost-call-limits-honest) wrote 25,000 `calls` rows in minutes. Consequence:
a synthetic panel run per hour writes hundreds of rows an hour with no ceiling, on a
disk that also holds the pre-deploy backup copy.

## Goals

- Every append-only table has a retention window; billing tables keep 400 days.
- Size is observable in `admin.usage` and `doctor`.
- The box never fills silently.

## Non-goals

- Archiving pruned rows off-box (the backup PRD keeps daily copies).
- Changing tenant quotas.

## User stories

- As the operator, I set `MCPHOST_RETENTION_CALLS_DAYS=90` and the nightly prune keeps
  the last 90 days of `calls`.
- As a tenant, `host.usage` still shows my last 30 days after a prune.
- As billing, metering rows for the last 400 days are untouched.
- As the disk guard, I return a clear error to `host.tool_call` when free space is
  under the floor rather than corrupting the file.

## Requirements

P0
1. Retention windows by env with defaults: `calls` 90 d, `calls_resource_usage` 90 d,
   `events` 30 d, `signup_events` 400 d, `metering` 400 d, `runs` (finished) 30 d,
   `threads/messages` 90 d; a table not listed is never pruned.
2. A nightly prune task in-process (`tokio` interval, 03:30 UTC, jittered) deletes in
   batches of 5,000 with `PRAGMA busy_timeout`, then `PRAGMA incremental_vacuum` when
   `auto_vacuum=INCREMENTAL` is set by a migration; logs rows deleted per table.
3. `admin.usage` gains `db_bytes`, `db_page_free_bytes`, `rows_by_table`, and
   `last_prune` (timestamp, deleted counts).
4. A disk guard: before a write path, when free space on the database's filesystem is
   under `MCPHOST_DISK_FLOOR_MB` (default 512), `host.tool_call`, `host.tool_publish`,
   and `signup` return `service_unavailable: disk floor` and `healthz` reports
   `disk_ok: false`.

P1
5. `host.usage` shows the tenant's retention windows.
6. Prune failures are journaled and surfaced in `healthz` as `last_prune_ok: false`.

P2
7. An admin tool `admin.prune_now` runs one cycle on demand.

## Success metrics

| metric | baseline | target | method | timeframe |
|---|---|---|---|---|
| database size trend | unmeasured | flat month over month at steady panel load | `admin.usage.db_bytes` daily | 60 d |
| writes refused for disk floor | n/a | 0 in normal operation, > 0 before corruption in a fill test | test | at ship |

## Technical considerations

- `rusqlite 0.32` bundled; `auto_vacuum` must be set before the first table exists,
  so the migration runs `VACUUM` once after setting the pragma (startup cost noted in
  the changelog).
- Prune must not hold the write lock longer than `busy_timeout` per batch.
- Metering retention is a billing constraint; 400 days covers annual invoicing.

## Migration / compatibility

The migration adds `retention_policy(table, days, updated_unix)` seeded from env; the
first prune deletes historic rows beyond the windows — the changelog names the count.

## Open questions

| question | owner | due |
|---|---|---|
| Defaults above (calls 90 d, events 30 d, metering/signup 400 d) | Joe | at build |

## Acceptance criteria

1. P0 — Given `calls` rows aged 91 and 89 days and `MCPHOST_RETENTION_CALLS_DAYS=90`, When the prune runs, Then the 91-day row is deleted and the 89-day row remains.
2. P0 — Given metering rows 399 days old, When the prune runs, Then none is deleted.
3. P0 — Given a table not in the policy, When the prune runs, Then its row count is unchanged.
4. P0 — Given 20,000 expired rows, When the prune runs, Then it deletes in batches and no concurrent `host.tool_call` fails with `database is locked`.
5. P0 — Given a completed prune, When `admin.usage` is read, Then `rows_by_table`, `db_bytes`, and `last_prune.deleted` are present and consistent with the deletion.
6. P0 — Given free space below the floor (simulated by a test hook), When `host.tool_call` runs, Then it returns `service_unavailable: disk floor` and `healthz` reports `disk_ok: false`.
7. P0 — Given prod after ship, When `admin.usage` is read on the following day, Then `last_prune` is younger than 24 h (proof: the live read in the trailer).
8. P1 — Given a tenant, When `host.usage` is called, Then the retention windows are listed.
9. P1 — Given a prune that errors, When `healthz` is read, Then `last_prune_ok` is false.
10. P2 — Given admin scope, When `admin.prune_now` is called, Then one cycle runs and returns its counts.
