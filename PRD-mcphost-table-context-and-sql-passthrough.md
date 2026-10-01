# PRD: mcphost-table-context-and-sql-passthrough — the database recipe drops its query grammar; tables carry notes and a query log

- Status: building
- Direct-build: laptop 2026-10-01 (session 6099ecef) — PR only, land via gate; dependent PRD-mcphost-query-diagnosis branches from this one's branch (feat/table-context-and-sql-passthrough)
- deferred_acs: [8, 9, 10]
- build_priority: high
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_version_bump: minor
- test_prefix: sqlpass
- publish: j0yen/private
- Vision: visions/mcphost-killer-apps.md
- Loop: mcphost-buildloop: satisfaction[database-in-a-minute]
- PM: Joe Yen
- Drafted: 2026-09-30
- Grounding: opportunity — visions/mcphost-killer-apps.md addendum 2026-09-30 (WWHTBT, weakest link leaf 3); not failure-derived. Audit of `src/tables.rs`, `src/kinds/mod.rs:939`, `examples/database-in-a-minute/tools/query.py`, `www/llms.txt` 555-590.
- Engineering target: mcphost `src/tables.rs` (schema output, query log), `src/handler.rs` (one new tool), `www/llms.txt` recipe section, `examples/database-in-a-minute/` (proof script, task, tools)

Absorbed scope: the "Give Claude a database in one minute" recipe shipped by PRD-mcphost-database-in-a-minute (built-prds, `Status: shipped`). That PRD's requirement 3 and AC4 made the recipe's `query` tool reject raw SQL and accept only a six-operator `where` grammar. This PRD reverses that decision on purpose: the recipe now asks questions as SQL through `host.table.query`, and the published `query` tool goes away. The shipped PRD is not edited (it is a landed contract); its AC4 is superseded here, not transferred.

## TL;DR

A person who loaded a CSV into mcphost can only ask it the questions a hand-written filter grammar anticipated: six operators, one table, no grouping, no pattern match. mcphost already has a real per-tenant SQLite store with a read-only SQL tool behind three independent guards, and a Python bridge to it, yet the recipe was written against the key-value store and wraps it in a grammar the model must learn from scratch. This PRD moves the recipe onto the SQL path, lets the tenant write one plain-text note per table that the cheap schema call returns, and records every query with its row count or refusal so the tenant can see which questions failed. Nothing new is invented for the model to speak; it writes SQL.

## Problem statement

**Who.** A person with a spreadsheet and a Claude subscription who followed the llms.txt recipe and now has an `expenses` table on mcphost (the `spreadsheet_owner` segment in the synthorg consumer corpus).

**What.** They want to ask their own rows any question a spreadsheet pivot could answer: "total by category", "which rows mention 'prod'", "average amount on days over 20".

**Why they cannot today.** The recipe's published `query` tool accepts `{columns?, where?: [{col, op, value}], limit?}` with `op` in `= != < > <= >=` and rejects everything else by design (`examples/database-in-a-minute/tools/query.py`, `ALLOWED_OPS`; llms.txt lines 567 to 575). There is no GROUP BY, no LIKE, no aggregate, no join. The grammar is private to this recipe, so the model has no training data for it; Paul Blankley's published numbers on domain-specific query languages put model accuracy roughly 80% below plain SQL for exactly this reason (Super Data Brothers, 2026, transcript at `~/Notes/transcripts/yt_g3Xq3MDFhcw.txt`). Meanwhile the host already ships what the recipe needs: `host.table.query` runs one read-only SELECT with CTEs, refused structurally by `sqlparser`, `PRAGMA query_only`, and `sqlite3_stmt_readonly`, bounded by `ROW_CAP` 1,000 and `QUERY_TIME_CAP` 5 s (`src/tables.rs` lines 34 to 69), and the Python sandbox exposes `mcphost.table.query(sql)` (`src/kinds/python.rs`, the `mcphost.table` section; `src/kinds/mod.rs:939`). llms.txt still tells tool authors that `mcphost.state.query` is "the only supported path from a python tool to your own table", which is false since PRD-mcphost-tenant-tables landed.

**Consequence.** The recipe's first two panel sessions scored satisfaction 0.000 on 2026-09-28 (vision, Recipe satisfaction, failure family `publish`). That row's family is the tool-publish step, which this PRD removes from the recipe entirely: with SQL available as a tenant tool, the recipe no longer needs to publish a Python wrapper at all. Separately, the model reads a table's meaning from nowhere: `host.table.schema` returns names, types, row count, and bytes only (`src/tables.rs:765`), and the description annotations `host.table.model_set` stores are returned only by the heavier `describe` call. Nothing records which questions a tenant's agent asked or which ones were refused, so neither the tenant nor the loop can tell a bad note from a bad query.

**Failure under this seed:** no. The seed is a design stance (Joe, 2026-09-30, agreeing with Blankley). The recipe's 0.000 satisfaction row belongs to the `publish` family, whose root (tenant key not carried into the builder session) is covered by synthorg run 302, not by a query grammar. This PRD removes the publish step from the recipe as a side effect; it does not claim to fix that family.

## Goals

1. The database recipe asks questions as SQL against `host.table.query`, with no published tool and no filter grammar.
2. A tenant can attach a plain-text note to a table and to a column, and `host.table.schema` returns it.
3. Every `host.table.query` call is recorded in the tenant's own table store with its row count, duration, and refusal code, readable through one tenant tool.
4. llms.txt tells the truth about the Python bridge.

## Non-goals

- `host.state.query` and its `field op value` filter grammar: unchanged. It is a key-value store's filter, not a query language for analytics.
- New annotation kinds: `model_set` already stores `role`, `unit`, `description`, `hidden`; this PRD returns `description` from `schema`, it adds no key.
- Retention or export controls for the query log beyond the fixed cap in requirement 3.
- Metering changes: a `host.table.query` call is already one tool call for billing.
- A control plane for the query log (admin views, alerts). The tenant-facing read is the capability; an admin surface is a later PRD if the loop asks for one.
- Changing the row cap, time cap, or the three read-only guards.

## User stories

1. **Spreadsheet owner.** After the import I ask "total amount by category" and get five rows back, without learning a query format.
2. **Spreadsheet owner.** I tell mcphost once that `amount` is in US dollars and `day` is the day of the month; from then on any agent that reads the schema sees that.
3. **Spreadsheet owner.** When an answer looks wrong I list the last ten queries, see the SQL and the row count each returned, and spot the one that hit the row cap.
4. **Recipe author (operator).** The llms.txt section is shorter than before, has no publish step, and the proof script exercises a GROUP BY and a LIKE the old grammar could not express.
5. **Tool author.** llms.txt tells me `mcphost.table.query(sql)` is available inside a `python` tool, so I stop routing through `mcphost.state`.
6. **Synthetic panel (loop).** The consumer task for this recipe replays the new llms.txt section and its satisfaction row fills on the next nightly.

## Requirements

### P0

1. **Schema returns notes.** `host.table.schema` returns `description` at table level and `description` per column whenever a matching annotation exists from `host.table.model_set` (key `description`, `column` omitted for the table). When no annotation exists the key is absent and the response shape is otherwise unchanged (`table`, `columns`, `rows`, `bytes_used`).
2. **Query log, written.** Every `host.table.query` call, whether it returns rows or is refused, appends one row to `_mcphost_query_log` inside the tenant's own `tables/<tenant_id>.db`: `id`, `created_unix`, `sql` (as submitted, at most 4,096 bytes stored), `row_count` (null on refusal), `duration_ms`, `error_code` (null on success; otherwise the same wire code the refusal returned, for example the parse, read-only, `row_cap`, or `time_cap` code), `error_message`. Logging failure never fails the query.
3. **Query log, bounded.** The log keeps the newest 1,000 rows per tenant; the insert that makes 1,001 evicts the oldest in the same transaction. The log's bytes count toward `bytes_used` and the plan's table-store quota exactly as rows do.
4. **Query log, read.** New tenant tool `host.table.query_log(limit?, before_id?)` returns rows newest first, `limit` defaulting to 50 and capped at 200, paging by `before_id`. It reads only the calling tenant's file; there is no argument that names another tenant.
5. **Recipe on the SQL path.** The llms.txt "database in a minute" section and `examples/database-in-a-minute/proof.sh` use `host.table.create`, `host.table.append`, one `host.table.model_set` description call, and `host.table.query` with SQL. No `host.tool_publish` step, no `where` grammar, no LIKE or raw-SQL rejection test. `examples/database-in-a-minute/tools/query.py` is deleted. The section stays under 70 lines and shows a GROUP BY question and a LIKE question.
6. **Consumer task updated.** `examples/database-in-a-minute/synthorg-task.yaml` keeps its id `spreadsheet-owner-database-in-a-minute` and describes the new steps and gold (a GROUP BY answer from the fixture), so the vision's satisfaction row continues under the same recipe name.
7. **llms.txt bridge line corrected.** The sentence claiming `mcphost.state.query` is the only path from a `python` tool to a table is replaced with the `mcphost.table.query(sql)` bridge.

### P1

8. **Refusals are diagnosable.** A logged refusal carries the same `error_code` and `error_message` the caller received, so a tenant reading the log can tell a row-cap hit from a parse error without re-running the query.
9. **Concurrent writers.** Ten `host.table.query` calls issued at once for one tenant produce ten log rows; the per-tenant connection's write lock serialises the appends.

### P2

10. **Log row hygiene.** `sql` longer than 4,096 bytes is stored truncated with a `truncated: true` flag on the row rather than refused.

## Success metrics

| metric | kind | baseline | target | method | timeframe |
|---|---|---|---|---|---|
| recipe satisfaction, database-in-a-minute | primary | 0.000 (2026-09-28, 2 sessions, family `publish`) | ≥ 0.75 on one nightly | vision Recipe satisfaction row, filled by the nightly panel | first nightly after land |
| proof.sh wall time against prod | secondary | under 30 s (predecessor AC6) | under 30 s with two more questions | proof stdout in the Live receipt | at land |
| queries refused by the read-only guards | guardrail | 0 non-SELECT executed (existing suite) | 0 | existing `tables` tests plus AC5 | every gate |
| query log overhead | guardrail | none (no log) | p95 added latency under 5 ms per query on the 1,000-row fixture | AC4 timing in the test | every gate |

## Technical considerations

- **Where the log lives.** In the tenant's own SQLite file next to `_mcphost_meta`, not in the main `mcphost.db`. The tables module's design promise is that one tenant's whole table store is one file to back up, restore, or delete (`src/tables.rs` lines 28 to 32); a log in the main database would break that and need a migration and a cascade. The `documents` module's `document_usage_events` pattern (migration 0037) is the wrong precedent here for that reason.
- **Reserved names.** `_mcphost_query_log` joins `_mcphost_meta` under the leading-underscore reservation `host.table.create` already enforces through its name regex, so a tenant cannot declare or drop it.
- **Where the notes live.** Annotations stay in the main database rows `upsert_table_model_annotation` writes today (`src/tables_model.rs:510`); `schema` reads them through `list_table_model_annotations` and merges only `description`. `describe` is unchanged.
- **Tool registration.** `host.table.query_log` sits with the other `host.table.*` tools in `src/handler.rs` (around line 1301) and dispatches to `tables::table_query_log`.
- **Timing.** `duration_ms` is measured around the existing execute-with-interrupt path so a `time_cap` refusal logs about 5,000.
- **Python bridge.** No change; `mcphost.table.query` already exists. The recipe simply stops needing a Python tool.
- **Proof script.** `proof.sh` already has `mcp_call`, `check`, and `has_error` helpers; the new questions reuse them. The fixture stays `fixture.csv`.

## Migration and compatibility

- Existing tenants' table files gain `_mcphost_query_log` lazily on their first `host.table.query` after deploy (`CREATE TABLE IF NOT EXISTS` on open, the same way `_mcphost_meta` is ensured).
- `host.table.schema` gains optional keys only; existing callers that ignore unknown keys are unaffected.
- Tenants who published the old `query` tool keep it; nothing deletes a published tool. The recipe no longer mentions it.
- The synthorg consumer task keeps its id, so the satisfaction history is continuous.

## Open questions

| question | owner | due |
|---|---|---|
| Should the query log be readable by an end user (`end_user_subject`) or only by the tenant key? Default here: tenant key only. | Joe | before land |
| Does the loop want `query_log` refusal counts as a failure family for the recipe? | mcphost-buildloop digest | first nightly after land |

## Acceptance criteria

1. P0 — Given a declared table with a table-level `description` annotation set through `host.table.model_set`, When `host.table.schema` is called, Then the response carries that text under `description` and the `table`, `columns`, `rows`, `bytes_used` keys are unchanged.
2. P0 — Given a column with a `description` annotation, When `host.table.schema` is called, Then that column's entry carries the text, and columns without one carry no `description` key.
3. P0 — Given a table with no annotations, When `host.table.schema` is called, Then the response has no `description` key at any level and matches the pre-change shape byte for byte.
4. P0 — Given a 1,000-row table, When `host.table.query` returns 5 rows for a GROUP BY, Then `_mcphost_query_log` gains one row with that `sql`, `row_count` 5, a `duration_ms` under 5,000, and null `error_code`, and the logged call added under 5 ms at p95 across 100 repetitions.
5. P0 — Given an `UPDATE` statement, When `host.table.query` refuses it, Then no table row changes, the caller gets the existing read-only refusal, and the log row carries that refusal's `error_code` with null `row_count`.
6. P0 — Given two tenants that each ran queries, When tenant A calls `host.table.query_log`, Then only A's rows return, newest first, and `limit` 500 is served as 200.
7. P0 — Given a tenant with 1,000 log rows, When one more query runs, Then the log holds 1,000 rows, the oldest is gone, and `host.table.schema`'s `bytes_used` includes the log.
8. P0 — Given the fixture loaded through `host.table.create` and `host.table.append`, When `proof.sh` runs against a local mcphost, Then the equality, range, count, GROUP BY sum by category, and LIKE `'%prod%'` answers each match the fixture's known values and no `host.tool_publish` call is made.
9. P0 — Given `www/llms.txt`, When the database-in-a-minute section is read, Then it is under 70 lines, shows create, append, one `model_set` description, a GROUP BY and a LIKE question through `host.table.query`, and `query_log`, and contains no `where` grammar and no claim that `mcphost.state.query` is the only bridge path.
10. P0 — Given prod after land, When `proof.sh https://mcphost.dev/mcp` runs from carbon with a fresh signup, Then it exits 0 in under 30 s and `host.table.query_log` on that tenant lists at least five rows with non-null `row_count` (Live; evidence: `mcphost-1: healthz version after deploy plus the proof stdout and the query_log response in the receipt under docs/receipts/`)
11. P1 — Given ten `host.table.query` calls issued concurrently for one tenant, When they finish, Then the log holds exactly ten new rows and every one has a `duration_ms`.
12. P1 — Given an empty declared table, When `SELECT * FROM t` runs, Then zero rows return and the log row carries `row_count` 0 and null `error_code`.
13. P2 — Given a 5,000-byte SELECT, When it runs, Then the log row stores the first 4,096 bytes with `truncated: true` and the query itself is not refused for length.
