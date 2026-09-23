# PRD: mcphost-webhook-inbox — every tenant gets an inbound URL that lands events in a table and wakes a tool

- Status: queued
- Lane: orch 2026-09-22T21:42:24.891404922+00:00 run=128
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_priority: normal
- publish: j0yen/private
- Cited-tree: mcphost@f59b60b
- Vision: visions/mcphost-killer-apps.md
- Grounding: opportunity — visions/mcphost-killer-apps.md, component 3; the only recipe that needs code (`src/triggers.rs` has kind=schedule only and `host.trigger.fire` is manual, no inbound URL)
- Depends-on: PRD-mcphost-tool-versions.md
- PM: Joe
- Drafted: 2026-09-18
- Engineering target: mcphost `src/triggers.rs` (new kind `webhook`), `src/handler.rs` (new public route), migrations (hook secret + deliveries table), `www/llms.txt` recipe section, `examples/webhook-inbox/` proof

## TL;DR

`host.trigger.set kind=webhook name=<n> tool=<t>` returns an inbound URL and a signing secret. A POST to that URL is verified, appended as a row to the tenant's `inbox_<n>` state table, and fires the bound tool with the event as its argument, reusing the bind-and-fire path mcphost-agent-wake built (v0.5x, "a tool bound to fire when a message arrives"). An agent that today needs a server to receive Stripe or GitHub events needs none. Free plan: 3 hooks (the existing `schedules=3` quota), 500 deliveries/day counted as calls.

## Problem statement

An indie agent builder (WHO) wants their agent to react to an external event, a payment, a push, a form submission (WHAT), and today must run and expose a web server because every hosted MCP is outbound-only, including mcphost whose triggers are schedule-only (WHY, `src/triggers.rs`). Consequence: the agent polls, or the builder pays for a box the agent does not otherwise need; on mcphost the trigger surface (`set/get/list/pause/resume/fire/test/replay/remove`) has one kind.

**Failure under this seed:** no — opportunity; a missing kind, not a defect.

## Goals

- One new trigger kind; the rest of the trigger surface (pause, resume, replay, test, remove) works on it unchanged.
- Verified deliveries only: HMAC over the body with the per-hook secret, replay-safe.
- Deliveries are rows the agent can query and a tool call it did not have to poll for.

## Non-Goals

- Provider-specific signature schemes (Stripe's `Stripe-Signature`, GitHub's `X-Hub-Signature-256`); P2 adapters, the P0 is mcphost's own HMAC plus an unverified mode that only stores.
- Outbound retries to the bound tool beyond the existing runs semantics.
- Public (unauthenticated) hooks without a secret.

## User stories

1. *Builder's agent* — I set a webhook trigger bound to `on_payment`; the URL goes into my Stripe dashboard; my tool runs on every event with the body as its argument.
2. *Builder's agent* — I query `inbox_payments` for the last hour's events without any tool having run.
3. *Builder* — I pause the hook; deliveries are stored but not fired; resume replays nothing by default, `host.trigger.replay` replays a chosen row.
4. *Joe* — abuse is bounded: an unknown URL returns 404 with no tenant leak; a bad signature is 401 and not stored; deliveries count against calls/day.

## Requirements

**P0**
1. `host.trigger.set` accepts `kind=webhook`, `name`, `tool`, optional `verify=hmac|none` (default hmac); returns `{url, secret}` once; `host.trigger.get` never returns the secret again.
2. Route `POST /hook/<opaque-id>` (id is not the tenant id) up to 256 KB body; verifies `X-Mcphost-Signature: sha256=<hmac>` when `verify=hmac`; 401 on mismatch, 404 on unknown id, 413 over size, 429 when the tenant is over calls/day.
3. Accepted delivery: insert `{id, received_at, headers (allowlisted), body}` into state table `inbox_<name>` (auto-created, counted in state quotas), then fire the bound tool through the same path agent-wake uses, with the row as the argument; the run is visible in `host.runs.list`.
4. `pause` stores without firing; `resume` does not auto-replay; `replay <row id>` fires once; `remove` drops the route and keeps the table.
5. Quotas: webhooks share the `schedules` quota (3 on free); each accepted delivery counts as one call.
6. Idempotency: a delivery carrying `X-Mcphost-Delivery-Id` seen in the last 24 h is 200 and not re-inserted.

**P1**
7. llms.txt section "Receive webhooks as an agent" and `examples/webhook-inbox/proof.sh`: fresh tenant, set hook, POST signed and unsigned bodies, assert table rows, run fired, 401 on unsigned, 404 on a wrong id, wall time printed.
8. `host.trigger.test` for a webhook kind sends a synthetic signed delivery.

**P2**
9. Provider adapters: `verify=stripe` and `verify=github` using the provider's header scheme and the hook secret.

Non-functional: accept-to-insert p95 under 200 ms on the ccx13; 100 concurrent deliveries to one hook lose none.

## Success metrics

| metric | baseline | target | method | timeframe |
|---|---|---|---|---|
| primary: real tenants with at least one accepted delivery | 0 | 5 | deliveries table joined to non-synthetic tenants | 30 days after ship |
| secondary: accept-to-fire latency | none | p95 under 1 s | runs `started_at` minus delivery `received_at` | first week |
| guardrail: unsigned delivery stored when `verify=hmac` | n/a | 0 | proof negative test | every Live run |

## Technical considerations

- Reuse the bind-and-fire path from mcphost-agent-wake rather than a second dispatcher; the trigger row gains `kind` and `hook_id` columns (migration), deliveries live in the tenant's state tables so retention (PRD-mcphost-data-retention) applies unchanged.
- The route must be outside bearer auth but inside the per-tenant quota accounting; the opaque id maps to tenant + trigger in one indexed lookup.
- Depends on tool-versions because a bound tool republished mid-stream should fire the pinned version (the vision's tool-versions migration note names `src/triggers.rs`).
- Body size and rate limits are the abuse surface; no delivery is ever executed as code, only stored and passed as an argument.

## Migration / compatibility

One migration (trigger kind + hook id + secret hash). Existing schedule triggers unaffected; `host.trigger.*` argument schemas gain optional fields only. Host-tool deprecation rules (PRD-mcphost-host-tool-deprecation) are not triggered since nothing is renamed.

## Open questions

| question | owner | due |
|---|---|---|
| Do deliveries count as calls (this draft) or as a separate `deliveries_per_day` quota | Joe | at build |
| Hook URL host: `mcphost.dev/hook/...` or a separate subdomain for isolation | Joe | at build |

## Acceptance criteria

1. P0 — Given `host.trigger.set kind=webhook name=pay tool=on_payment`, When it returns, Then the response has `url` and `secret`, and `host.trigger.get` afterwards has the url and no secret.
2. P0 — Given a body signed with the secret, When POSTed to the url, Then 200, one row in `inbox_pay`, and one run of `on_payment` whose argument equals the row.
3. P0 — Given an unsigned or wrongly signed body, When POSTed, Then 401 and no row.
4. P0 — Given an unknown opaque id, When POSTed, Then 404 with an empty body.
5. P0 — Given a 300 KB body, When POSTed, Then 413 and no row.
6. P0 — Given the hook paused, When a signed body is POSTed, Then 200, a row, and no run; `resume` creates no run; `replay <row>` creates exactly one.
7. P0 — Given the same `X-Mcphost-Delivery-Id` twice within 24 h, When POSTed, Then the second is 200 and the table has one row.
8. P0 — Given a free tenant with 3 schedule triggers, When it sets a webhook, Then it is refused with the schedules quota error.
9. P0 — Given 100 concurrent signed deliveries to one hook, When they complete, Then the table has 100 rows and 100 runs exist.
10. P1 — Given the fix deployed to mcphost.dev, When `examples/webhook-inbox/proof.sh` runs against it with a fresh tenant, Then every assertion in AC1–7 holds and wall time is printed. (Live; evidence: proof stdout in the receipt)
11. P1 — Given `host.trigger.test` on a webhook trigger, When called, Then one synthetic signed delivery is stored and fired.
12. P2 — Given `verify=stripe` and a body signed the Stripe way with the hook secret, When POSTed, Then it is accepted.
