# PRD: mcphost-admin-schema-contract — the admin listings carry a schema the deploy tool pins

- Status: queued
- Lane: orch 2026-09-24T07:04:07.667912491+00:00 run=171
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_priority: high
- publish: j0yen/private
- Vision: visions/grand-loop.md
- Depends-on: PRD-mcphost-deploy-measure-tenant-shape.md
- Loop: grand-loop: instrument family
- PM: Joe
- Drafted: 2026-09-18
- Grounding: failure-derived — visions/grand-loop.md addendum 2026-09-18 (wwhtbt leaf 3, the weakest link): two repos share the admin listing shape and neither tests it
- Engineering target: mcphost `src/admin.rs` (`admin.tenants`, `admin.usage`), new `schemas/admin/*.json`, mcphost-deploy conformance test

## TL;DR

`admin.tenants` and `admin.usage` are read nightly by mcphost-deploy and by the grand
loop, and their row shape changed under a release with no test on either side. Each
listing gets a `schema_version`, a JSON Schema committed in the mcphost repo, a test in
mcphost that every emitted row validates, and a conformance test in mcphost-deploy that
pins the version it understands.

## Problem statement

Two repos share one contract and neither writes it down. Evidence: the 2026-09-16
`KeyError: 'namespace'` (visions/grand-loop.md chain, level 3) followed the 0.57.0 change
to the admin tenants listing (CHANGELOG, self-offboard AC5); mcphost's 306 test files
assert tool behavior, not the listing's row schema; mcphost-deploy's tests use fixture
rows. Consequence: any admin listing change is a silent break of the revenue read.

## Goals

- One schema file per admin listing, versioned, validated in mcphost's own tests.
- The deploy tool refuses a listing whose `schema_version` it does not know, with a
  clear message, instead of a traceback later.

## Non-goals

- Versioning the tenant-facing `host.*` surface (PRD-mcphost-host-tool-deprecation).
- Changing what the listings contain.

## User stories

- As mcphost-deploy, I read `schema_version: 1` and validate rows against the pinned
  schema before computing anything.
- As the mcphost builder, a change to a listing's row fails a test until I bump the
  version and the schema together.
- As Joe, `doctor` tells me "admin schema 2, deploy tool knows 1" before the timer fires.

## Requirements

P0
1. `admin.tenants` and `admin.usage` responses carry top-level `schema_version`
   (integer, starts at 1) beside the existing rows.
2. `schemas/admin/tenants.v1.json` and `schemas/admin/usage.v1.json` (JSON Schema
   draft 2020-12) are committed; every optional field (offboard marker, attribution)
   is declared with its type and nullability.
3. A mcphost test builds a database with a normal tenant, a disabled tenant, a
   self-offboarded tenant, and a harness-prefixed tenant, calls both listings, and
   validates every row with the `jsonschema` crate already in `Cargo.toml`.
4. mcphost-deploy (in the same dispatch, as a cross-repo companion change recorded in
   the archive trailer) vendors the schema files, validates rows before
   `compute_measure`, and exits 5 with `admin schema <n> unsupported (known: <list>)` on
   an unknown version.

P1
5. `mcphost-deploy doctor` prints the live `schema_version` and the vendored versions.
6. A CHANGELOG lint in mcphost: a diff touching `src/admin.rs` listing fields without a
   schema file change fails `cargo test`.

P2
7. `host.whoami` reports `admin_schema_version` for tenants with admin scope.

## Success metrics

| metric | baseline | target | method | timeframe |
|---|---|---|---|---|
| admin listing changes that break the deploy read | 1 in 3 days | 0 | demand ledger alarms attributed to shape | 30 days |
| listing row fields undocumented | all | 0 | schema coverage test | at ship |

## Technical considerations

- `jsonschema 0.26` is already a dependency; no new crate.
- The schema files are the contract; the deploy tool copies them at build with a
  recorded source commit.
- Cross-repo landing: this PRD's `build_into` is mcphost; the mcphost-deploy change is
  small and is listed in the trailer with its commit (PRD-build-cross-repo-commit-gate
  governs the push).

## Migration / compatibility

Consumers that ignore `schema_version` keep working (additive). The deploy tool's
validator ships with version 1 only.

## Open questions

| question | owner | due |
|---|---|---|
| Should `schema_version` bump on additive changes (assumed: no, additive fields are optional in the schema) | Joe | at build |

## Acceptance criteria

1. P0 — Given a call to `admin.tenants`, When the response is read, Then it carries integer `schema_version` = 1 and rows unchanged otherwise.
2. P0 — Given a database with normal, disabled, self-offboarded, and harness tenants, When both listings are validated against their v1 schema, Then every row validates.
3. P0 — Given a row with an unexpected additional field of unknown type, When validated with `additionalProperties: false` in the schema, Then the test fails naming the field.
4. P0 — Given mcphost-deploy pinned to v1 and a listing reporting `schema_version: 2`, When `measure` runs, Then it exits 5 with the unsupported-version message and writes no `measure.json`.
5. P0 — Given prod after this ships, When `mcphost-deploy doctor` runs on RedBaron, Then it prints the live schema version equal to a vendored one (proof: doctor output in the trailer).
6. P1 — Given a change to a listing field in `admin.rs` with no schema change, When `cargo test` runs, Then the changelog-lint test fails.
7. P2 — Given an admin-scoped tenant, When `host.whoami` is called, Then `admin_schema_version` is present.

- iter_log: 2026-09-23T10:40-07:00 operator note (Joe via Claude) — build_priority set to high; order: deploy-measure-tenant-shape → admin-schema-contract → human-claim-magic-link (market-test gates 1 and 7)
