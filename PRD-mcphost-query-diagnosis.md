# PRD: mcphost-query-diagnosis — a refused or empty query gets a hint that names the missing context

- Status: building
- Direct-build: laptop 2026-10-01 (session 6099ecef) — PR only, land via gate; branched from PRD-mcphost-table-context-and-sql-passthrough's branch (feat/table-context-and-sql-passthrough), stacked PR
- build_priority: normal
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_version_bump: minor
- test_prefix: qdiag
- deferred_acs: [10]
- publish: j0yen/private
- Vision: visions/mcphost-aistack-harvest.md
- Depends-on: PRD-mcphost-table-context-and-sql-passthrough.md
- Loop: mcphost-buildloop: satisfaction[database-in-a-minute]
- PM: Joe Yen
- Drafted: 2026-09-30
- Grounding: opportunity — visions/mcphost-aistack-harvest.md (WWHTBT leaf 5); parts: j0yen/mcp-grounding-eval (`CoverageReport`, `EntityResult`, `strsim::jaro_winkler`), ~/projects/ai-stack @ 4ef6c22 mcp-interaction-scorer (metric definitions), mqo-session-footprint-meter (`tokens_from_chars`). mcphost `_mcphost_query_log` from the dependency, `src/tables.rs` sqlparser gate, `src/tables_model.rs` annotations.
- Engineering target: mcphost `src/tables.rs` (log columns, diagnosis at log time), `src/query_diag.rs` (identifier coverage), `src/handler.rs` (two tools)

## TL;DR

Once the query log exists, a tenant can see that a query was refused or returned nothing, but not why. Blankley's observability loop needs the next step: find the missing context. mcp-grounding-eval already scores the entities a question names against a model surface with a string-similarity match and a coverage band. This PRD applies the same scoring to the identifiers a SQL statement names against the tenant's tables, columns, and description notes, stores a hint on the log row at log time, and adds per-tenant query statistics with an estimated token cost per result. The hint tells the agent "column `amout` not found, did you mean `amount`" without a second round trip.

## Problem statement

**Who.** An agent whose query was refused or came back empty, and the spreadsheet owner reading the query log to see what went wrong.

**What.** Know whether the failure was a misspelled or missing column, a table that does not exist, a filter that matched nothing, or a cap.

**Why they cannot today.** After PRD-mcphost-table-context-and-sql-passthrough, `_mcphost_query_log` records `sql`, `row_count`, `duration_ms`, `error_code`, `error_message`. A parse or bind error from SQLite says "no such column: amout" and nothing more; a zero-row result says nothing at all. The tenant's schema and its `description` annotations are available in the same process. mcp-grounding-eval's `CoverageReport {coverage, band, entities: [{term, status: covered|fuzzy_covered|partial|absent, matched_to, top_candidates: [{name, similarity}]}]}` over `strsim::jaro_winkler` is the exact shape, built for a different surface (measures and dimensions from `describe_model`). mcp-interaction-scorer defines the session metrics that matter: retry rate, empty-result rate, bind-failure rate.

**Consequence.** The agent retries blind; the tenant cannot tell a bad note from a bad query; the loop cannot cluster failures by cause.

**Failure under this seed:** no. Opportunity; see the vision. The dependency is a hard build order: this PRD reads a table the predecessor creates.

## Goals

1. Every refused or zero-row query carries a stored hint naming the closest existing identifier when there is one.
2. A tenant can ask for query statistics over a window: counts by outcome, latency percentiles, estimated result tokens.
3. The diagnosis is deterministic and needs no model call.

## Non-goals

- Rewriting or re-running the query for the caller.
- Semantic diagnosis of a wrong-but-valid query (the row count is right, the question was wrong). Out of reach without a model.
- Changing the query log's retention or shape beyond the added columns here.
- Admin or cross-tenant statistics.

## User stories

1. **Agent.** My query failed; the log row's hint says `amout` is absent and `amount` matches at 0.96, so I fix it in one step.
2. **Agent.** My query returned zero rows; the hint says the filter value `'Produce'` has no exact match in `category` and the closest values are `produce` and `product`.
3. **Spreadsheet owner.** I call `query_stats` for the last day and see 40 queries, 3 refused for row cap, 5 empty, p95 120 ms.
4. **Loop digest.** The nightly reads `query_stats` per tenant and clusters refusals by `error_code` into failure families.

## Requirements

### P0

1. **Identifier extraction.** For every query the sqlparser gate accepts or rejects with a bind error, `src/query_diag.rs` collects the table names, column names, and string-literal filter values the statement names (from the AST on success, from the error text plus a best-effort parse on failure).
2. **Coverage scoring.** Each identifier is scored against the tenant's declared tables and their columns, plus `description` annotation text as extra candidate names, with Jaro-Winkler similarity: `covered` at 1.0, `fuzzy_covered` at ≥ 0.9, `partial` at ≥ 0.75, `absent` below. Output per identifier: `term`, `kind` (`table`|`column`|`value`), `status`, `matched_to`, `top_candidates` (≤ 3 `{name, similarity}`).
3. **Hint at log time.** When a query is refused with a parse or bind error, or returns zero rows, the log row gains `diagnosis` (JSON: the identifier results and `band`: `covered`|`partial`|`absent`) and `hint` (one sentence, ≤ 200 characters, e.g. "column `amout` not found in `expenses`; closest: `amount` (0.96)"). Successful non-empty queries store null for both. Diagnosis never delays the query response by more than 5 ms p95 and never fails it.
4. **Value diagnosis for empty results.** For a zero-row query with an equality filter on a text column, the diagnosis checks the literal against the column's distinct values (bounded to 1,000 distinct) and reports the closest three.
5. **Tool: diagnose.** `host.table.query_diagnose(log_id)` returns the stored `diagnosis` and `hint` for one log row, recomputing them against the current schema if the row predates this feature.
6. **Tool: stats.** `host.table.query_stats(window_s?)` (default 86,400, max 604,800) returns `{queries, ok, empty, refused: {error_code: n}, p50_ms, p95_ms, result_rows_total, est_result_tokens_total, top_hints: [≤5 {hint, count}]}` for the calling tenant.

### P1

7. **Footprint columns.** Log rows gain `result_bytes` and `est_tokens` (`ceil(result_bytes / 4)`, the footprint meter's default), so `query_stats` can report cost.
8. **Bridge.** `mcphost.table.query_diagnose(log_id)` and `mcphost.table.query_stats(window_s=None)` in the python sandbox.

### P2

9. **Retry detection.** `query_stats` reports `retries`: consecutive queries by the same tenant within 60 s whose normalised SQL differs by one identifier that the earlier diagnosis flagged.

## Success metrics

| metric | kind | baseline | target | method | timeframe |
|---|---|---|---|---|---|
| refused or empty queries carrying a non-null hint | primary | 0 (no field) | ≥ 90% of rows with a parse/bind error or zero rows | query log | first nightly after land |
| hint correctness on the fixture | guardrail | n/a | 100% of AC cases name the intended identifier | AC1–AC4 | every gate |
| added latency per query | guardrail | 0 | ≤ 5 ms p95 | AC3 timing | every gate |
| database recipe satisfaction | secondary | vision recipe row | ≥ prior nightly | vision | first nightly after land |

## Technical considerations

- **Dependency.** Reads and extends `_mcphost_query_log` created by PRD-mcphost-table-context-and-sql-passthrough; the added columns are applied with `ALTER TABLE … ADD COLUMN` on open, idempotently, in the tenant's own file.
- **Similarity.** `strsim` (already a transitive dependency? if not, add it; it is small and pure). Thresholds are constants with the grounding-eval defaults.
- **Schema snapshot.** Tables and columns come from the per-tenant file's `_mcphost_meta`; description annotations from `list_table_model_annotations`. One read per diagnosed query; cached per call.
- **Bind errors.** SQLite's "no such column: X" and "no such table: X" messages are parsed for X; the AST from sqlparser (which succeeds for well-formed SQL that binds wrongly) supplies the rest.
- **No model call, no network.**

## Migration and compatibility

- Additive columns on a tenant-file table; old rows read as null and are recomputed on demand by `query_diagnose`.
- `query_log` output gains `hint`; callers ignoring unknown keys are unaffected.

## Open questions

| question | owner | due |
|---|---|---|
| Should `top_hints` feed a failure family in the synthorg digest automatically? | mcphost-buildloop digest | first nightly after land |
| Expose thresholds per tenant? Default: no. | Joe | before land |

## Acceptance criteria

1. P0 — Given table `expenses` with column `amount`, When `SELECT amout FROM expenses` is submitted, Then the query is refused, the log row's `diagnosis` marks `amout` as `fuzzy_covered` matched to `amount` with similarity ≥ 0.9, and `hint` names both.
2. P0 — Given no table named `expense`, When `SELECT * FROM expense` is submitted, Then the diagnosis marks the table `fuzzy_covered` to `expenses` and the hint says so.
3. P0 — Given `SELECT * FROM expenses WHERE category = 'Produce'` returning zero rows, When diagnosed, Then the value `Produce` is `absent` with `produce` among `top_candidates` and the hint names it, and the query's own latency rose by at most 5 ms p95 across 100 runs.
4. P0 — Given a successful query with rows, When its log row is read, Then `diagnosis` and `hint` are null.
5. P0 — Given a log row id, When `host.table.query_diagnose` is called by the owning tenant, Then it returns that row's diagnosis; when called by another tenant, Then `not_found`.
6. P0 — Given 40 logged queries in the last hour with 3 `row_cap` refusals and 5 empty results, When `host.table.query_stats(3600)` is called, Then counts match, `p95_ms` is computed from the rows, and `refused.row_cap` is 3.
7. P0 — Given a column with a `description` annotation "amount in USD", When `SELECT usd FROM expenses` is refused, Then the diagnosis offers `amount` as a candidate through the annotation text.
8. P1 — Given a 200 KB result, When logged, Then `result_bytes` is within 1% of the serialised size and `est_tokens` equals `ceil(result_bytes / 4)`.
9. P1 — Given the python sandbox, When a tool calls `mcphost.table.query_stats()`, Then it receives the same object the tool returns.
10. P0 — Given prod after land, When a fresh tenant loads the fixture and submits `SELECT amout FROM expenses`, Then `host.table.query_log` shows the row with a hint naming `amount` (Live; evidence: `mcphost-1: healthz version after deploy plus the query_log response in the receipt under docs/receipts/`)
11. P1 — Given ten concurrent refused queries, When they finish, Then all ten log rows carry a hint and none is missing a diagnosis.
