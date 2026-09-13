# PRD: mcphost-client-attribution-default-leak — client_name/client_version fabricate "rmcp 3.2.0" for callers that sent no clientInfo

- Status: queued
- Depends-on: PRD-mcphost-gate-debt-c627803.md
- Lane: redbaron 2026-09-13T17:07:05Z pid=438344 boot=c6865fd1-71c2-48cf-818e-5e1f2246b3fe
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_priority: normal
- publish: j0yen/private
- Vision: visions/mcp-host.md
- Loop: grand-loop: paid_mrr_usd — the goal metric is real revenue; a metrics surface that cannot distinguish real from synthetic cannot detect the first real customer
- Grounding: stress-QA finding from PRD-homeward-mcp-tenant (2026-09-13) — observed directly against live mcphost.dev via host.whoami and the `tenants` table.
- PM: Joe
- Drafted: 2026-09-13
- Engineering target: j0yen/mcphost tenant-attribution capture path (the mcphost-tenant-attribution feature, 2026-09-08)

## TL;DR

`host.whoami` and the `tenants` table report `client_name: "rmcp"`, `client_version: "3.2.0"` for a caller that sent zero MCP `clientInfo` — a bare single-shot JSON-RPC POST with no `initialize` handshake, the exact shape mcphost's own streamable-HTTP transport is documented to tolerate. "rmcp" happens to be the name of the Rust MCP SDK mcphost itself (and homeward-mcp) are built on: the attribution capture path is very likely defaulting to the server's own SDK identity instead of `null`/`unknown` when the request carries none. This silently fabricates specific-looking client attribution — directly undermining the signal mcphost-tenant-attribution (and this whole tenant-onboarding dogfood) exists to produce.

## Problem statement

PRD-homeward-mcp-tenant's real onboarding run (2026-09-13, tenant `t_ea9749c3`, display_name `pawsandpetals`) called `signup` and `host.whoami` via a raw `httpx` POST (no rmcp library, no `initialize`, per `stress_protocol.py`'s documented wire contract: "mcphost's streamable-HTTP surface tolerates a single-shot POST with no prior initialize handshake or session id"). `host.whoami` nonetheless returned `client_name: "rmcp"`, `client_version: "3.2.0"`. The live `tenants` table confirms the same pair on the only other `origin: external` row (`fleet-adapter-ops`, id 387) — both real, non-synthetic tenants carry an identical, specific-looking client identity neither caller actually sent.

Five whys:

1. Why does `host.whoami` report `client_name: "rmcp"` for a caller that sent no `clientInfo`? — Directly observed: a bare httpx POST (no MCP client library) got back `client_name: "rmcp"`, `client_version: "3.2.0"` in both `host.whoami`'s response and the persisted `tenants` row.
2. Why would the server fill in a specific name/version instead of leaving it null? — mcphost (and homeward-mcp) are built on the `rmcp` Rust SDK; the attribution capture path most likely reads a compile-time/library constant as a fallback when the inbound request's `clientInfo` is absent, rather than branching to an explicit "unknown" sentinel.
3. Why wasn't this caught by mcphost-tenant-attribution's own test suite (6 tests, `attrib_ac1-6`, landed 2026-09-08)? — Those tests most likely exercise requests that DO send proper `clientInfo` (the common SDK-client shape); the specific "no initialize, single-shot `tools/call`" transport path — the one `stress_protocol.py` itself calls out as a deliberately distinct, documented-tolerated shape — appears untested for attribution correctness specifically.
4. Why does that transport shape exist at all, rather than being rejected? — It's a deliberate design choice (documented, verified against a real build) for load-generation/concurrency: no shared session object serializing concurrent callers. Legitimate feature; the bug is only in what attribution defaults to under it.
5. Why does this matter for the tenant-onboarding vision specifically? — The entire point of onboarding a genuine tenant is producing a "non-synthorg, externally distinguishable" signal in tenant accounting (the 2026-09-08 "tenants_real is synthetic" finding this vision exists to close). A client identity that silently fabricates "rmcp 3.2.0" for every handshake-skipping caller produces false precision that could mislead a future audit of real client diversity — the opposite of what distinguishability is for.

## Goals

- `client_name`/`client_version` are `null` (or an explicit `"unknown"` sentinel, not the server's own SDK identity) whenever the inbound request's `clientInfo` is genuinely absent.
- The fix is at the attribution capture site (the deepest actionable level per the five-whys above), not a shallow mask on `host.whoami`'s output alone — `admin.usage`/any other surface reading the same column must see the same correction.
- A regression test pins the exact transport shape that exposed this (single-shot `tools/call`, no prior `initialize`, no `Authorization` clientInfo) against a real or realistic fixture, so this doesn't silently reappear.

## Non-Goals

- No change to the deliberately-tolerant single-shot transport behavior itself — that's a feature, not the bug.
- No retroactive correction of already-persisted rows (`t_ea9749c3`, `t_a711d0fd`, and any others) — out of scope; this PRD fixes capture going forward. A follow-up data-cleanup PRD can be drafted separately if Joe wants historical rows corrected.
- No broader MCP clientInfo validation/spec-compliance work beyond this one default-value defect.

## User stories

- As the operator (Joe) auditing tenant attribution for real-vs-synthetic signal, a `client_name: null` tells me "this caller sent no clientInfo" instead of a specific-looking SDK name that isn't true.
- As a future stress/audit script reading `tenants.client_name`, I can trust the field: either a real reported client identity, or an explicit absence marker — never the server's own SDK leaking through.

## Requirements

- P0 — Locate the attribution capture path (likely near where `mcphost-tenant-attribution`'s migration-0010 columns are populated) and change the no-clientInfo fallback from the `rmcp`/SDK-version constant to `null`.
- P0 — `host.whoami` and any `admin.*` tenant-listing tool reflect the corrected (nullable) value, not a re-derived string.
- P1 — A new test reproduces the exact bug shape: a single-shot `tools/call` (no `initialize`) against a real or integration-fixture mcphost, asserting `client_name`/`client_version` come back null/unknown, not the SDK's own identity.
- P1 — `CHANGELOG.md` entry naming the defect and citing this PRD.

## Acceptance criteria

1. P0 — Given a single-shot `tools/call` request with no prior `initialize` and no `clientInfo`, When the tenant record is read back (`host.whoami` or equivalent), Then `client_name`/`client_version` are null/unknown, not `rmcp`/any SDK-version string.
2. P0 — Given a request that DOES send proper MCP `clientInfo` (e.g. a real `initialize` handshake from an SDK client), When the tenant record is read back, Then the real reported client name/version are captured unchanged (no regression on the working case).
3. P1 — Given the new regression test, When run in CI, Then it fails against the pre-fix code path and passes after the fix (verified both ways during this PRD's own build).
