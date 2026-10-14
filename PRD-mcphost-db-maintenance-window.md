# PRD — mcphost-db-maintenance-window: the host optimises, checkpoints and rotates at its own quietest hour, computed from `calls`, with before/after sizes in the ledger and a skip while a backup runs

- Status: queued
- Lane: redbaron 2026-10-13T20:16:55.740279806+00:00 run=780
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- publish: j0yen/mcphost
- build_priority: normal
- test_prefix: dbmw
- Grounding: creative (Joe 10-09 "take the leap")
- Falsifier: after seven nightly windows, the count of `database is locked` tick failures and `sqlite_write_ms` red minutes on the following days is not lower than the seven days before — maintenance moves nothing
- iter_log: 2026-10-11T14:11:37Z amended by /dream (creative-ac1-premise-checks-to-live) — AC 1 rewritten as a Live AC (its premise check needs prod/orch/truth-tier data the coder box cannot reach; 8 runs ended unpaired on AC 1 on 10-11); original text preserved under "## Premise check (operator)"
- Vision: visions/mcphost-self-healing.md
- Depends-on: PRD-mcphost-heal-action-registry-and-ledger.md
- Loop: synthorg-chain: explore live_call_error
- PM: Joe
- Drafted: 2026-10-09
- Engineering target: `src/db.rs` (`wal_checkpoint_passive` 2525, `spawn_wal_checkpoint_loop` 18634, `wal_checkpoint_secs_from_env` 18617, `PRAGMA page_count` 2499), `src/retention.rs` (`spawn_prune_scheduler` 388, disk floor 331-366), `src/statusfeed.rs` (`daily_tick_once` 505, `next_daily_tick_unix` ~520), `src/heal.rs`, new `src/maintenance.rs`, `migrations/0001_init.sql:33` (`calls.started_unix`)

## TL;DR

The host already checkpoints the WAL on a fixed interval (`spawn_wal_checkpoint_loop`, PASSIVE) and prunes retention daily, each at whatever moment its timer lands — including inside the midnight chain. This PRD adds `src/maintenance.rs` and two actions: `HealAction::DbMaintenance` (`PRAGMA optimize`, `PRAGMA wal_checkpoint(TRUNCATE)`, `PRAGMA incremental_vacuum` when `freelist_count` > 5 % of pages) and `HealAction::LogRotate` (the host's own `scratch/` and `exports/` sweep plus a `journalctl --vacuum-size` request flag for the deploy tool). Both run once a day at the lowest-traffic hour computed from the last 14 days of `calls.started_unix`, write `before/after {db_mb, wal_mb, freelist_pages, scratch_entries, journal_mb}` to the ledger, skip with `outcome: refused` while a backup is in progress, and never delete a tenant row.

## Problem statement

**Who:** the agent whose 3 am write waits on a 5.7 MB WAL being checkpointed under load; the chain, whose midnight hour is the busiest of the day (journal warn/error peak 6480 lines at 07:00 UTC vs 127 at 15:00); Joe, who runs `VACUUM` by hand never.
**What:** routine database and disk hygiene at the hour nobody is there, with evidence.
**Why not today:** `wal_checkpoint_passive` (`db.rs:2525`) runs every `MCPHOST_DB_WAL_CHECKPOINT_SECS` regardless of load and PASSIVE never truncates under readers; `PRAGMA optimize` is never run (grep: 0 hits); prod `mcphost.db` is 349 MB with a 5.7 MB WAL and 239 entries in `scratch/`, the journal holds 1.6 GB; 10-07 13:21-13:22 UTC two background ticks failed `database is locked`. The retention pruner has a disk floor but no notion of a quiet hour.
**Premise (agent viewpoint):** "the host tidies up when I am not there, and my writes at peak are faster for it." Falsifier: the quiet-hour computation picks the chain hour (AC 1 feeds a recorded 14-day `calls` histogram and must pick 15:00 UTC, the week's real minimum).

## Goals / Non-goals

Goals: quiet-hour computation from `calls`; `DbMaintenance` and `LogRotate` executors with before/after evidence; backup-in-progress skip; `/status.json.maintenance`; dry run; max 1 per action per 24 h (expressed as `max_per_hour: 1` plus a daily schedule).
Non-goals: full `VACUUM` (rewrites the file, needs 2× disk, blocks writers — refused forever); deleting tenant rows (retention PRDs own that); backups (PRD-mcphost-db-offbox-backup, built); changing `busy_timeout`.

## User stories

1. As Joe, `/incidents.json` at 7 am shows `db_maintenance 11:02 UTC (ok, 1.8 s): wal 5.7→0 MB, freelist 1 240→0 pages, optimize ran` and `log_rotate 11:02 UTC (ok): scratch 239→12 entries, exports 0 removed, journal_vacuum_requested: true`.
2. As an autonomous coding agent, nothing changes for me except `/status.json.maintenance.next_unix` telling me the window, so I can avoid scheduling a heavy import at that minute.
3. As the off-box backup job, I set the `backup_in_progress` marker (`/var/lib/mcphost/.backup-lock`) and the window skips with `refused: backup in progress` instead of racing my copy.
4. As the chain's five-whys, I compare `sqlite_write_ms` red minutes before and after the window and see the falsifier either way.

## Requirements

P0
- R1. `maintenance::quiet_hour(db, days = 14) -> u8`: the UTC hour with the fewest `calls` rows summed over the last 14 days, ties broken by the later hour; recomputed daily; when `calls` has fewer than 14 × 24 rows, default 11:00 UTC (4 am PDT, the current minimum neighbourhood).
- R2. `DbMaintenance` executor, via `heal::execute`: `before` = `{db_mb, wal_mb, page_count, freelist_count}`; run `PRAGMA optimize`, `PRAGMA wal_checkpoint(TRUNCATE)` (retry PASSIVE once if busy), `PRAGMA incremental_vacuum(<pages>)` only when `freelist_count > 0.05 × page_count` and `auto_vacuum = INCREMENTAL`; `after` = same keys + `optimize_ms, checkpoint_result`. Each statement has a 30-s deadline; on timeout the row reads `failed` with the statement name.
- R3. `LogRotate` executor: remove `scratch/` entries older than 24 h and `exports/` entries past their expiry (both already have TTLs the host knows); never touch `mcphost.db*`, `envs/`, `public/`, `authz_signing_key.der`; `after.journal_vacuum_requested: true` sets a flag in `/status.json.maintenance` the deploy tool's next probe acts on (`journalctl --vacuum-size=500M` needs root).
- R4. Both actions skip with `outcome: refused, reason: "backup in progress"` when `<data_dir>/.backup-lock` exists and is younger than 2 h, and are retried at the next hour; a lock older than 2 h is logged stale and ignored.
- R5. `/status.json.maintenance`: `{quiet_hour_utc, next_unix, last: {at, outcome, db_mb, wal_mb} | null, journal_vacuum_requested}`.
- R6. Policy: `max_per_hour: 1` for each; the scheduler fires once per day at `quiet_hour_utc:02`; `mcphost maintenance run --dry-run` prints the plan with current `before` values and writes a `would_run` row.

P1
- R7. The window also runs `statusfeed::prune_once` and the retention pruner's daily pass if they have not run in the last 20 h, so all housekeeping shares the hour.

## Success metrics

| metric | baseline (7 d to 10-09) | target | method | when |
|---|---|---|---|---|
| `database is locked` tick failures | 2 | 0 | prod journal grep | 7 nights after landing |
| WAL size at chain start (07:00 UTC) | 5.7 MB observed 10-09 | < 1 MB | `/status.json.maintenance.last.wal_mb` + probe `wal_size_bytes` | 3 chains |
| maintenance inside the chain window | unknown (timer-aligned) | 0 | row `at` vs chain ledger | 3 chains |

## Technical considerations

- `PRAGMA optimize` is cheap on a WAL-mode database with up-to-date stats; the first run may take seconds on 349 MB — the 30-s deadline and the ledger `optimize_ms` make it measurable.
- `incremental_vacuum` only works when `auto_vacuum` was set before the first table — if prod reports `auto_vacuum = 0`, the row records `freelist_count` and `vacuum_skipped: "auto_vacuum=0"` and the Open question decides.
- Quiet-hour query: `SELECT (started_unix / 3600) % 24 AS h, count(*) FROM calls WHERE started_unix > ?1 GROUP BY h` on the prune-role connection (`retention.rs:269` pattern) so it never contends with the server role.
- `.backup-lock` is a convention the deploy tool's `backup.py` adopts in a one-line change; until then the window checks for a `mcphost-backup` process name as a fallback.

## Migration / compatibility

No schema change. `spawn_wal_checkpoint_loop` keeps its interval (PASSIVE checkpoints remain the steady state); TRUNCATE happens only in the window.

## Open questions

| question | owner | due |
|---|---|---|
| Prod `auto_vacuum` is almost certainly 0; enabling INCREMENTAL needs one full `VACUUM` under a maintenance deploy. Do it once by hand, or accept freelist growth? | Joe | after the first week's `freelist_count` rows |
| Should the chain's start time move off the quiet hour computation (chain = peak by design)? Default: no; the chain is the host's busiest tenant and that is the point. | Joe | after 3 nights |

## Acceptance criteria

1. P0 — Given this PRD has landed, When the operator runs the premise check recorded under "## Premise check (operator)" below, Then its result is written into this PRD and a failed premise parks the PRD (Live; evidence: operator premise check, see "## Premise check (operator)")
2. P0 — Given a test database with a 3 MB WAL and 1 000 freelist pages under `auto_vacuum = INCREMENTAL`, When `DbMaintenance` runs, Then `after.wal_mb` is 0, `after.freelist_count` is 0, `checkpoint_result` is `TRUNCATE`, `outcome` is `ok` and `optimize_ms` is a positive integer.
3. P0 — Given `<data_dir>/.backup-lock` created 10 min ago, When the window fires, Then both rows read `refused` with `reason: "backup in progress"`, nothing is checkpointed, and the next attempt is scheduled 3600 s later; Given the lock is 3 h old, Then the actions run and a tracing line says `stale backup lock ignored`.
4. P0 — Given `scratch/` holds 5 entries older than 24 h and 3 younger, plus `exports/` with one expired export, When `LogRotate` runs, Then exactly 5 + 1 entries are removed, `mcphost.db`, `mcphost.db-wal`, `envs/`, `public/` and `authz_signing_key.der` are untouched (hash-compared), and `after.journal_vacuum_requested` is `true`.
5. P0 — Given a reader holding a long transaction, When `wal_checkpoint(TRUNCATE)` is busy, Then the executor retries PASSIVE once, the row reads `ok` with `checkpoint_result: PASSIVE`, and no statement exceeded its 30-s deadline.
6. P0 — Given `mcphost maintenance run --dry-run`, When it runs, Then it prints `before` values for both actions, writes two `would_run` rows and changes no file or pragma (hash-compared).
7. P1 — Given the retention pruner last ran 21 h ago, When the window fires, Then the pruner's daily pass runs inside it and its own log line appears between the two maintenance rows' timestamps.
8. P0 — Given this PRD deployed to prod and one quiet hour elapsed, When `/status.json` is fetched, Then `maintenance.last.outcome` is `ok` and `maintenance.last.wal_mb` is below 1 (Live; evidence: `curl -s https://mcphost.dev/status.json | jq -c .maintenance`)

## Premise check (operator)

Original AC 1 (10-09 dream; Live since 2026-10-11 because it needs prod, orch, truth-tier run directories or the conformance ledger, which the coder box cannot reach; the operator runs it after land and records the result here): Given a `calls` table seeded with the 14-day hourly histogram recorded from prod on 10-09 (`tests/fixtures/dbmw_calls_histogram.json`), When `quiet_hour` runs, Then it returns the histogram's true minimum hour and that hour is not 07 (the chain hour); Given fewer than 336 rows, Then it returns 11.
