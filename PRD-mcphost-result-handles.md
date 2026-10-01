# PRD: mcphost-result-handles — hold a large result server-side and query it again in SQL

- Status: building
- Direct-build: laptop 2026-10-01 (session 6099ecef) — PR only, land via gate
- build_priority: normal
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_version_bump: minor
- test_prefix: handle
- publish: j0yen/private
- Vision: visions/mcphost-aistack-harvest.md
- Loop: mcphost-buildloop: satisfaction[database-in-a-minute]
- PM: Joe Yen
- Drafted: 2026-09-30
- Grounding: opportunity — visions/mcphost-aistack-harvest.md (WWHTBT leaf 4); parts from ~/projects/ai-stack @ 4ef6c22: dh-spec `DatasetSummary`, dh-summary `summarize`, dh-store TTL+LRU; dh-vs-json-bench as evidence. mcphost `src/tables.rs` (`ROW_CAP` 1000 line 63, `tenant_db_path` line 231, `open_conn` line 246), `src/plans.rs:60-61`, `src/export.rs`.
- Engineering target: mcphost `src/tables.rs` (materialise, summarise, evict), `src/handler.rs` (three tools), `src/plans.rs` (one quota field), `src/export.rs` (CSV export reuse)

## TL;DR

A query that matches more than 1,000 rows is refused, so an agent cannot work with a large result at all, and a result under the cap is returned whole into the model's context. ai-stack's dataset-handle crates held results server-side behind an opaque id with a bounded summary and a TTL, and its benchmark showed why: a model asked to compute over raw JSON gets arithmetic wrong that a query gets right. This PRD rebuilds the handle as a table inside the tenant's own SQLite file. The model asks for a handle, reads a 20-row sample and per-column stats, and writes further SQL against the handle. No operator grammar: SQL is the only way to touch the rows.

## Problem statement

**Who.** An agent working a spreadsheet owner's table, or a `python` tool that needs an intermediate result.

**What.** Run a query that yields 50,000 rows, look at its shape, then aggregate or filter it further without pulling the rows through the model.

**Why they cannot today.** `host.table.query` refuses any result over `ROW_CAP` (1,000) by design (`src/tables.rs:63`), and everything under the cap is serialised as one JSON array into the tool result, where a model reads it as text. There is no server-side place to keep a result. Temporary tables in SQLite would work, but the connection is per call and nothing names or expires them. ai-stack's `dh-spec::DatasetSummary {row_count, columns, sample, sample_cap, stats, notes}`, `dh-summary::summarize` (bounded sample, stats never dropped), and `dh-store` (TTL plus LRU, size cap, derive lineage) define the shape; `dh-vs-json-bench` documents the failure modes of the alternative.

**Consequence.** Any multi-step analysis either re-runs the base query per step or is impossible above the cap. The database recipe's satisfaction row cannot rise past questions that fit in one SELECT.

**Failure under this seed:** no. Opportunity; see the vision.

## Goals

1. A query can materialise its result as a handle with no row cap, under a byte quota and the time cap.
2. The handle's summary is bounded and honest: a sample, and stats over all rows.
3. Later SQL references the handle by name, through the same read-only guards.
4. Handles expire on their own and are listable, droppable, and exportable.

## Non-goals

- An operation kernel (`aggregate`, `filter`, `pivot` as tool arguments). Not ported. SQL over the handle does this.
- Cross-tenant or cross-file handles.
- Persisting a handle past 24 hours. Copy it to a declared table with `CREATE TABLE AS` if it should live.
- Streaming or paging the raw rows of a handle through the tool result.

## User stories

1. **Agent.** I run the base query with `handle: true`, get `hdl_…`, its row count of 48,212, and stats per column, then `SELECT region, AVG(amount) FROM hdl_… GROUP BY region`.
2. **Agent.** I list my handles, see one is about to expire, and drop the rest.
3. **Spreadsheet owner.** I ask for a CSV of the handle and get a download link.
4. **Python tool author.** `mcphost.table.query(sql, handle=True)` gives me a name I can pass to the next step of a chain.
5. **Operator.** A tenant on the free plan cannot fill the disk with handles; the quota refuses the materialisation and names the limit.

## Requirements

### P0

1. **Materialise.** `host.table.query(sql, handle: true, ttl_s?)` runs the SELECT through the existing guards and, instead of collecting rows, executes `CREATE TABLE hdl_<id> AS <sql>` inside the tenant's own file, where `<id>` is 12 lowercase base32 characters. `ROW_CAP` does not apply; `QUERY_TIME_CAP` does. Metadata (`created_unix`, `expires_unix`, `sql`, `bytes`, `row_count`) is a row in `_mcphost_meta`.
2. **Summary.** The call returns `dataset-summary.v1`: `{handle, table, row_count, columns: [{name, dtype, nullable}], sample: [≤20 rows], sample_cap: 20, stats: {col: {min, max, sum, mean, distinct, top_k: [≤5 {value, count}]}}, bytes, expires_unix, derived_from: sql}`. Stats are computed by SQL over the whole handle, never from the sample; `sum` and `mean` only for numeric columns.
3. **Query the handle.** Any later `host.table.query` may reference `hdl_<id>` in `FROM` or a CTE; the parser gate, `PRAGMA query_only`, and the read-only statement check apply unchanged. A handle name that does not exist or has expired returns `handle_not_found` naming it.
4. **TTL and eviction.** Default `ttl_s` 3,600, maximum 86,400. An expired handle is dropped by the tables tick within 60 s of expiry. A new plan field `table_handle_bytes_max` (free: 64 MiB) caps the tenant's live handle bytes; materialising past it evicts the least-recently-queried handles first, and if the new handle alone exceeds the cap the call is refused with `handle_quota_exceeded` naming the cap.
5. **List and drop.** `host.table.handles()` returns live handles newest first with `handle`, `row_count`, `bytes`, `created_unix`, `expires_unix`, `last_used_unix`, `derived_from`; `host.table.handle_drop(handle)` drops one.
6. **Reserved names.** `hdl_` is a reserved prefix: `host.table.create` and `host.table.drop` refuse it.

### P1

7. **Export.** `host.table.handle_export(handle)` writes the handle as CSV through the existing export job and returns the signed `/exports/{run_id}` URL (24 h).
8. **Python bridge.** `mcphost.table.query(sql, handle=True, ttl_s=None)`, `mcphost.table.handles()`, `mcphost.table.handle_drop(h)`.
9. **Query log.** Once PRD-mcphost-table-context-and-sql-passthrough has landed, a materialisation logs one row with `row_count` and the handle name in `error_message` left null. No `Depends-on`; if the log table is absent nothing is logged.

### P2

10. **Notes.** `notes` on the summary carries the table-level `description` annotations of the source tables the SQL referenced, when present.

## Success metrics

| metric | kind | baseline | target | method | timeframe |
|---|---|---|---|---|---|
| queries refused for `row_cap` per tenant-day | primary | current count from the query log after sql-passthrough lands | −80% for tenants that use handles | query log | first two nightlies after land |
| handle materialise time, 50,000 rows × 5 columns | secondary | n/a | under 2 s on the builder | AC test | every gate |
| handle bytes over quota | guardrail | n/a | 0 | AC5 | every gate |
| stale handles past expiry + 60 s | guardrail | n/a | 0 | AC4 | every gate |

## Technical considerations

- **Same file.** Handles live in `tables/<tenant_id>.db` so backup, restore, and delete stay one file; the tables module's doc header states that promise. They count toward `bytes_used` reported by `host.table.schema` and `host.table.list`.
- **Materialisation is a write** on a connection that otherwise runs `PRAGMA query_only = ON`; the materialise path opens its own connection without the pragma, validates the SELECT with sqlparser first, and wraps `CREATE TABLE AS` plus the meta row in one transaction.
- **Stats in SQL.** One `SELECT` per column for min, max, sum, mean, distinct; one `GROUP BY … LIMIT 5` for top_k. Bounded by the handle's size; a 50,000-row handle stays under a second.
- **Eviction order.** `last_used_unix` updates on every query that references the handle (the parser gate already yields table names).
- **No new migration** in the main database; the plan field is a `plans.rs` constant per plan, matching how `table_rows_max` is defined (`src/plans.rs:61`).

## Migration and compatibility

- Additive tools and one argument. Existing `host.table.query` calls are unaffected.
- Soft order: land after PRD-mcphost-table-context-and-sql-passthrough to avoid two concurrent edits in `src/tables.rs`. Not a `Depends-on`.

## Open questions

| question | owner | due |
|---|---|---|
| Separate `table_handle_bytes_max` or share the table byte pool? Default here: separate, free 64 MiB. | Joe | before land |
| Should `handle_export` require the pro plan? Default: any plan. | Joe | before land |

## Acceptance criteria

1. P0 — Given a table with 50,000 rows, When `host.table.query` runs a SELECT over all of it with `handle: true`, Then a `dataset-summary.v1` returns with `row_count` 50000, a 20-row sample, per-column stats computed over all rows (the test recomputes `sum` and `distinct` by SQL), and the call finishes under 2 s on the builder.
2. P0 — Given that handle, When `SELECT category, SUM(amount) FROM hdl_<id> GROUP BY category` runs through `host.table.query`, Then the rows match the same aggregate over the source table.
3. P0 — Given a handle name that was never created, When it is referenced, Then `handle_not_found` names it and no rows return.
4. P0 — Given `ttl_s: 1`, When 61 s pass and the tick has run, Then the handle is gone from `host.table.handles`, its table is dropped from the file, and `bytes_used` fell accordingly.
5. P0 — Given a free-plan tenant with 60 MiB of live handles, When a 10 MiB materialisation runs, Then the least-recently-queried handles are evicted until it fits, and when a single 70 MiB materialisation runs, Then `handle_quota_exceeded` names the 64 MiB cap and nothing is created.
6. P0 — Given `host.table.create` with name `hdl_abc`, When called, Then a validation error states the prefix is reserved.
7. P0 — Given an `UPDATE hdl_<id> …` statement, When submitted to `host.table.query`, Then it is refused by the existing read-only guard and the handle is unchanged.
8. P1 — Given a handle, When `host.table.handle_export` is called, Then a signed `/exports/{run_id}` URL returns CSV with `row_count` data rows plus a header within 24 h and 410 after.
9. P1 — Given a `python` tool, When it calls `mcphost.table.query(sql, handle=True)` then queries the handle, Then both calls succeed inside the sandbox.
10. P0 — Given prod after land, When a fresh tenant loads the 1,000-row fixture and materialises `SELECT * FROM expenses` as a handle then queries `SELECT COUNT(*) FROM hdl_<id>`, Then the count is 1000 and `host.table.handles` lists the handle with its expiry (Live; evidence: `mcphost-1: healthz version after deploy plus the three tool responses in the receipt under docs/receipts/`)
11. P1 — Given two tenants, When tenant A references tenant B's handle name, Then `handle_not_found` and no rows.
12. P0 — Given an empty result, When materialised, Then the handle exists with `row_count` 0, an empty sample, and stats with `distinct` 0 per column.
