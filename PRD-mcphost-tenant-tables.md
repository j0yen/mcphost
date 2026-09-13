# PRD: mcphost-tenant-tables — the warehouse the data-pipeline segment keeps asking for

- Status: building
- Lane: redbaron 2026-09-13T09:58:37Z pid=439336 boot=c6865fd1-71c2-48cf-818e-5e1f2246b3fe
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_priority: normal
- build_version_bump: minor
- publish: j0yen/private
- test_prefix: tables
- Vision: visions/mcp-host.md
- Grounding: failure-derived — warehouse task failed 4 of 4 comparable runs (visions/mcp-host.md, Iceberg trigger); held component pulled forward by operator 2026-09-12
- Loop: mcphost-buildloop: satisfaction
- PM: Joe Yen
- Drafted: 2026-09-12
- Blocked: gate: reviewer-agent — missing: /home/jsy/wintermute/mcphost/target/autobuilder/receipts/reviewer-agent.json; gate: ci-checks — workflow(s) on HEAD are not green, or still pending (the next tick retries); gate: flake-audit — verdict=block not in ["pass", "skipped"]. Fresh `extend-gate.sh` run at head=8b34db16e6c0ee8bceaebf8ab5199246278d496c: receipts=25 pass=22 block=3, verdict=block; `extend-gate: delta verdict=block baseline=present new_blocks=reviewer-agent,ci-checks,flake-audit inherited_blocks=none` — all three blocking receipts are in-scope (not baseline debt), so this PRD's own diff must clear them before shipping. `gate-debt.sh check` ack'd tracking (consecutive=1, elapsed=0s; threshold not yet met, no follow-on PRD drafted). Receipt: `~/brain/journal/build/receipts/2026-09-13-mcphost-tenant-tables-gate1-block.txt`. Did NOT tag, archive, or write a passing Receipts line. next: gate-red.
- Engineering target: extend `~/wintermute/mcphost` — a per-tenant table store with `host.table.*` tools, plan quotas, and access from python-kind tool code; ai-stack (`~/repos/ai-stack`) is the parts bin for Iceberg-facing pieces, reused only where a crate drops in

## TL;DR

`data-pipeline-builder-warehouse-table-status` has failed on every comparable run —
0.12.0, 0.27.0 twice, 0.30.0 twice — because the task needs a warehouse and there is
none: personas invent Databricks hostnames that answer nothing, and seven live
production tools carry the literal placeholder `{{workspace_host}}` (2026-09-09 audit).
Joe pulled the held Iceberg component forward on 2026-09-12. This PRD ships the
usable slice: per-tenant tables — create with a schema, append rows, query read-only
SQL — as `host.table.*` tools and from inside a python tool's code, quota'd per plan.
The brief's full vision (auto-built semantic model over a warehouse's existing tables)
stays held; this gives agents a warehouse-shaped surface that exists, so the
four-sighting task class finally measures the host instead of a dead URL.

## Problem statement

Data-pipeline agents build tools that read and write tabular state, and mcphost offers
them nothing between the key-value store (tenant-state, small values) and "bring your
own warehouse" (which synthetic personas answer by inventing
`adb-1234567890.12.azuredatabricks.net` — vision, 2026-09-09, 12 sessions of 84 lost
to placeholder upstreams). The capability panel's data segment was the one segment
willing to switch on a modest improvement (switch share 0.33), the corpus has carried
warehouse-shaped tasks since v0.4.1, and the "fails for want of" trigger the vision
set for this component has now fired on four consecutive comparable runs. The
consequence is measured: data_pipeline_builder satisfaction 0.0 at 0.12.0 and still
bottom-tier at 0.30.0, always for the same reason — the task cannot be completed on
this host.

## Goals

- A tenant can create a table with a declared schema, append rows, and run read-only
  SQL against its own tables, from the agent session (`host.table.*`) and from its
  tool code, with no external warehouse.
- Quotas per plan bound tables, rows, and bytes, with the structured
  quota-exceeded-names-upgrade-path error shape billing already established.
- The warehouse task class in the corpus is completable on-host.

## Non-goals

- No auto-built semantic model over the tables (the brief's end vision; returns when
  usage shows table shapes worth modeling).
- No cross-tenant tables, sharing, or external engine endpoints (JDBC/ADBC).
- No document embeddings (the other held component; still no failing task).
- No arbitrary write-SQL: writes go through `table.append` (typed, quota-checked),
  never through the query surface.
- No replacement of tenant-state (small KV stays KV).

## User stories

1. **Data-pipeline agent.** When my tool ingests rows each run, I want to append to a
   table I created and query it later, so my pipeline holds state without a paid
   external warehouse.
2. **Monitor-pattern agent.** When my scheduled tool compares this run to history, I
   want SQL over my own past rows, so "what changed" is a query, not a JSON file I
   parse in code.
3. **Operator.** When a tenant's tables grow, I want plan quotas to bound disk with
   the same upgrade-path errors calls and tools already use, so storage cannot become
   an unmetered cost.

## Requirements

**P0**
1. `host.table.create` (name, column schema from a small type set: text, integer,
   real, timestamp, boolean, json), `host.table.append` (rows validated against the
   schema), `host.table.query` (read-only SQL, single statement, bounded rows and
   execution time), `host.table.list`/`drop` — all tenant-scoped, all following the
   envelope contract, all metered as calls.
2. Read-only enforcement on `query` is structural (parse-level rejection of non-SELECT
   statements and multi-statements), not string matching; violations are structured
   errors.
3. Tool-code access: a python tool reaches the same four operations over the existing
   sandbox channel (the tenant-state channel precedent — one channel, one more message
   family), covering the scheduled-monitor story without the agent in the loop.
4. Quotas per plan: max tables, max rows per table, max total bytes per tenant;
   exceeding returns the structured error naming `billing.checkout`; usage appears in
   `host.usage`.
5. Isolation: a tenant can never name, query, or join another tenant's tables; the
   admin cascade (`admin.tenant_delete`) removes tables with the tenant.
6. Docs: descriptors, quickstart, README, llms.txt — including one sentence on
   table-store vs key-value store choice.

**P1**
7. `host.table.schema` returns a table's schema and row/byte counts, so an agent can
   discover its own state without querying.
8. Backing storage lives under `$MCPHOST_DATA_DIR` with per-tenant accounting the
   deploy backup already covers (restore drills must not need new steps).

**P2**
9. Iceberg-format export: `host.table.export` writes a table snapshot in an
   open-format file the tenant can fetch — the bridge toward the brief's Iceberg
   commitment, reusing an ai-stack writer crate only if one drops in cleanly.

## Success metrics

| metric | baseline | target | method | timeframe |
|---|---|---|---|---|
| warehouse task class completable on-host | failed 4/4 comparable runs | corpus task passes against local instance | tables ACs + next comparable run | first run after ship + corpus proposal |
| data_pipeline_builder satisfaction | bottom segment every run | movement measured at next candidate run | lift vs standing baseline | after corpus task adoption |
| Placeholder upstreams in live tools | 7 `{{workspace_host}}` tools | new data tools use host.table.* | production tools audit | first month |

## Technical considerations

- Storage engine: SQLite-per-tenant (a file per tenant under the data dir) is the
  shape that fits the host's existing SQLite operational story, backup coverage, and
  the delete cascade; a shared DB with row scoping is the alternative — decide in
  build for isolation strength, and document the choice.
- The corpus change (a task that uses `host.table.*`) ships as a `promote-usecase`
  proposal, never auto-appended — the fingerprint re-anchor rule holds; note the
  baseline re-anchor requirement in the proposal.
- Query bounds reuse the call-limits conventions (row cap, time cap, output cap all
  named in errors — the call-limits-honest lineage).
- ai-stack is a parts bin, not a substrate (standing lineage ruling): lift a crate if
  one fits (P2 export), never a dependency on its engine.

## Migration / compatibility

Additive migration for table metadata + quota columns; no change to existing tools or
kinds. Rollback drops the new tools and leaves tenant table files inert on disk.

## Open questions

| question | owner | due |
|---|---|---|
| SQLite-per-tenant vs shared-scoped store (isolation vs ops simplicity) | build | at build, documented in the PRD receipt |
| Free-plan quota numbers (tables/rows/bytes) | Joe | at build |
| When the auto-semantic-model layer returns (usage-triggered, per the brief) | Joe | after first month of table usage |

## Acceptance criteria

1. P0 — Given a tenant, When it creates a table, appends 3 valid rows, and queries them with a SELECT and a WHERE, Then results return under `result.payload` with correct values and types.
2. P0 — Given an append whose row violates the schema (wrong type, missing column, unknown column), When submitted, Then it is refused with a structured error naming the column and rule, and no partial rows land.
3. P0 — Given `query` receives an UPDATE, a DROP, a multi-statement string, and a SELECT joining another tenant's table name, When each runs, Then each is refused structurally and the refusal names the rule; the cross-tenant name reads as nonexistent, never as forbidden-but-present.
4. P0 — Given a plan with max 2 tables and a row cap, When the tenant creates a third table or appends past the cap, Then the structured quota error names `billing.checkout`, and `host.usage` reports table counts and bytes.
5. P0 — Given a python tool whose code appends and queries via the sandbox channel, When called, Then the operations succeed under the same tenant scoping and quotas as the session tools.
6. P0 — Given a query returning more than the row bound, or running past the time bound, When executed, Then it terminates with a structured error naming the bound, and the host serves the next call normally.
7. P0 — Given `admin.tenant_delete` on a tenant with tables, When it completes, Then the tenant's table storage is gone (cascade verified on disk), and no other tenant's tables changed.
8. P0 — Given the build completes, When llms.txt and descriptors are read, Then all table tools and the KV-vs-table sentence are present.
9. P1 — Given a table with rows, When `host.table.schema` is called, Then it returns columns, types, row count, and byte count without executing a query.
10. P1 — Given a backup/restore cycle via the existing deploy drill against an instance with tenant tables, When restore completes, Then the tables and rows survive with no drill changes required.
11. P2 — Given a table with rows, When `host.table.export` runs, Then an open-format snapshot file is produced and fetchable by the tenant, and its row count matches the table.
