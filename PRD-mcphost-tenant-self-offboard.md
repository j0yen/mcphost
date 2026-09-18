# PRD: mcphost-tenant-self-offboard — a tenant can leave the way it joined

- Status: building
- Lane: redbaron 2026-09-18T04:14:45Z pid=2508817 boot=c6865fd1-71c2-48cf-818e-5e1f2246b3fe
- Operator-note: 2026-09-17 9:10 pm EDT (Joe: requeue) — gate-red block RESOLVED as stale: hermetic-build failed only because the 09-16 tick forced BURST_LANE=1 through hcloud ssh sockets; the burst box is deleted and burst routing is off, so it no longer reproduces. Re-gate on RedBaron. Original: gate-red — extend-gate.sh (scope=branch) blocks on the hermetic-build extended-receipts producer because this tick's mandatory BURST_LANE=1 routes cargo through hcloud+ssh sockets that rustbuild's hermetic_build.rs ignore_rules allowlist (loopback, unix-domain only) does not cover; root cause confirmed by source read, no fix landed in ~/wintermute/rustbuild as of this dispatch; fix is out of scope for this PRD (build_into=mcphost only). AC1-5 are fully implemented, committed, and independently re-verified green (703 tests pass) on branch autobuilder/mcphost-tenant-self-offboard — nothing left 
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_priority: high
- publish: j0yen/private
- Vision: visions/mcp-host.md
- Loop: grand-loop: paid_mrr_usd — a tenant that cannot leave through a public path is a tenant the promotion HOLD's real-integration signal can't trust
- Grounding: stress-QA finding from PRD-homeward-mcp-tenant (2026-09-13) — real public onboarding run (tenant `t_ea9749c3`, display_name `pawsandpetals`) found no public tenant-deletion path while exercising the full lifecycle.
- PM: Joe
- Drafted: 2026-09-13
- Engineering target: j0yen/mcphost `host.*` tool surface (new self-service lifecycle tool), `tenants`/`signup_events` disable semantics

## TL;DR

A tenant that signed up through the public path (`signup(name)`) has no public way to close its own account: the anonymous `tools/list` catalog (enumerated live against mcphost.dev 2026-09-13, 413 signup_events rows, tenant `t_ea9749c3`) offers `host.tool_remove`, `host.state.delete*`, `host.trigger.remove` — all scoped to sub-resources — but nothing that disables or deletes the tenant record itself. Only `admin.*` (gated by `MCPHOST_ADMIN_KEY`, confirmed via `/etc/mcphost/env` on mcphost-1) can flip a tenant's `disabled` flag. A real customer has no self-serve offboarding.

## Problem statement

PRD-homeward-mcp-tenant's AC6 required "a documented rollback: the tenant can be removed via public/account paths without residue in billing" as direct evidence that a stranger's full lifecycle — not just signup — works without admin help. Walking the real tool catalog against the live mcphost.dev deployment (tenant `t_ea9749c3`, free plan, `source_class: external`) turned up no such tool. Five whys:

1. Why can't a tenant remove itself via a public path? — No tool in the live, anonymously-enumerable `tools/list` catalog (host.*, billing.*, signup) does it; only `admin.*` tools (separately keyed, not shown to anonymous/tenant callers) write the `disabled` column.
2. Why does only `admin.*` own tenant lifecycle writes? — Tenant-management write paths were scoped to admin as the default-safe posture when tenant-attribution/billing features (mcphost-tenant-attribution, 2026-09-08) were built; self-service lifecycle was outside those PRDs' contracts.
3. Why was self-service deletion never added since? — Every tenant before this PRD's run was either a synthorg persona (284 rows, `origin: synthorg:mcp-host-project-consume`) or an internal harness/ops probe (`fleet-adapter-ops`, `harness:unstamped` rows) — nobody needed to leave, so the gap was invisible.
4. Why didn't billing/attach work surface this? — Those PRDs' ACs covered attribution correctness and billing correctness, not account lifecycle; offboarding was never in scope, not an oversight within their own contracts.
5. Why does it matter now? — Promotion is on HOLD specifically pending real-integration signal (Joe, 2026-09-09); a SaaS with no way for a real customer to close their own account is exactly the kind of gap that HOLD exists to catch before promotion lifts, not after.

## Goals

- A tenant holding only its own `tenant_key` can deactivate/delete its own account through a public tool call — no admin key, no operator ticket.
- Offboarding leaves no billing residue: a disabled tenant cannot be billed further, and (for a `pro` tenant) any Stripe subscription is canceled, not just the local row toggled.
- The action is destructive-by-design but safe: idempotent, and the tenant's key is invalidated so a leaked key can't be used to "un-delete" anything.

## Non-Goals

- No data-export/GDPR-erasure pipeline — this PRD handles deactivation, not scrubbing historical signup_events rows (those stay for audit, same as today's admin-disabled tenants).
- No change to `admin.*` tenant management — additive only.
- No UI — this is a tool-call product; stays that way.

## User stories

- As a tenant (the stranger who signed up), I call `host.self_offboard()` with my own key and my account, tools, and secrets are gone — same channel I joined through.
- As the operator (Joe), I see the tenant's row flip to disabled with a recorded reason (`self_offboard`), distinguishable from an admin-initiated disable.

## Requirements

- P0 — New tool `host.self_offboard(tenant_key)`: disables the tenant (`disabled=1`), invalidates the key (future calls with it get the same "unknown/disabled tenant" error as a never-issued key), and removes published tools/secrets/state per existing retention conventions for a disabled tenant.
- P0 — If the tenant's plan is `pro` with an active Stripe subscription, the subscription is canceled (live-mode) as part of the same call — no billing continues after offboard.
- P0 — The call is idempotent: calling it twice (or with an already-disabled tenant's key) returns a clean typed result, not an error that looks like corruption.
- P1 — `host.whoami` / `billing.status` after offboard return a typed "tenant disabled" error rather than 500ing or returning stale data.
- P1 — An admin-visible audit field (e.g. `origin_detail` or a new `disabled_reason`) distinguishes self-offboard from admin-disable.

## Acceptance criteria

1. P0 — Given a live free-plan tenant created via public `signup`, When it calls `host.self_offboard(tenant_key)` with its own key, Then the tenant's row shows `disabled=1` and the call returns success.
2. P0 — Given an already-offboarded tenant, When `host.self_offboard` is called again with the same key, Then it returns a clean idempotent result, not a crash or ambiguous error.
3. P0 — Given an offboarded tenant's key, When any other `host.*`/`billing.*` tool is called with it, Then a typed "unknown or disabled tenant" error returns, same shape as an unissued key.
4. P0 — Given a `pro`-plan tenant with a live Stripe subscription, When it calls `host.self_offboard`, Then the Stripe subscription is canceled (verify via Stripe live-mode API/dashboard) and no further invoice is generated.
5. P1 — Given the live deployment, When an operator queries `tenants` for a self-offboarded row, Then it is distinguishable from an admin-disabled row (reason field populated).

- iter_log: 2026-09-15T21:28:00Z build_priority set by operator (Joe, carbon: "prioritize the build-skill prds") — casper PRDs high, the three widened mcphost PRDs fill idle slots only
- iter_log: 2026-09-15T22:24:30Z amended by /dream (Joe 2026-09-15 "go 4 wide on different branches", "prioritize the build-skill prds") — build_priority normal→high: AC6 blocker of homeward-mcp-tenant (a different build tree); shipping it opens a parallel lane
