# PRD: mcphost-human-claim-magic-link — the human behind the agent claims the namespace by email

- Status: queued
- Lane: orch 2026-09-23T06:23:07.583719053+00:00 run=144
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_priority: high
- publish: j0yen/private
- Vision: visions/mcphost-market-test.md
- Cited-tree: mcphost@609f122903757fd85ee23e6f6c6bba4135b59344
- Loop: mcphost-buildloop: satisfaction — claimed tenants per external signup
- PM: Joe
- Drafted: 2026-09-22
- Engineering target: mcphost (src/control.rs, src/http.rs, src/db.rs, www/)

## TL;DR

`signup` returns a `claim_url` alongside the key. A human opens it, enters an email, clicks a magic link, and the tenant gains a verified owner. The claim page shows what the agent built (tools, schedules, webhooks, calls today, state size) and offers the Pro upgrade with the email pre-filled. Admin healthz counts claimed tenants. Email goes out through a provider named by environment variables, with a fake client in tests.

## Problem statement

Joe's decision (2026-09-23): the buyer is the human, the agent is the channel. Today the namespace belongs to whoever holds the bearer key or redeems the single-use handoff token (src/control.rs:178-198, `host.redeem` at src/control.rs:233); no route, page, table column, or email dependency exists for a person to own a tenant (src/http.rs:449-473 routes; www/ has no account page; Cargo.toml has no mail crate). Consequence: two tenants, both the operator's, $0 revenue, and no way for a stranger's agent to turn activation into a paying human. Billing checkout (src/billing.rs) has no customer email to attach.

## Goals

- A human can claim a tenant from a link the agent hands over, in under two minutes, with nothing installed.
- Ownership is verified (email magic link), single-winner, and visible to the operator.
- The claim page is the first human-facing view of "what my agent set up".

## Non-goals

- Password accounts, sessions, or a full dashboard beyond the claim summary.
- Transferring or revoking ownership (a later PRD).
- OAuth for MCP clients.

## User stories

- Agent operator: my agent signs up for me while I sleep; in the morning it tells me "claim your backend at this link", and I see what it built.
- Agent operator: I click the link on my phone, type my email, tap the emailed link, done. No CLI.
- Agent operator: from the claim page I upgrade to Pro and the receipt goes to the email I just verified.
- Operator (Joe): I see how many external tenants were claimed, by signup source, in admin healthz.
- Agent: the signup result and llms.txt tell me exactly what to say to my human.

## Requirements

P0
1. `signup` response gains `claim_url` (absolute, `https://<host>/claim/<claim_token>`), in both key and handoff modes; the claim token is single-use, 32 bytes of randomness, expires after `MCPHOST_CLAIM_TOKEN_TTL_SECS` (default 7 days).
2. `GET /claim/{token}` renders a page with an email form; `POST /claim/{token}` with a syntactically valid email sends a magic link and renders "check your inbox"; invalid or empty email re-renders with an error and sends nothing.
3. `GET /claim/verify/{code}` (single-use, 30-minute TTL) sets `tenants.owner_email`, `tenants.owner_verified_at`, consumes the claim token, and renders the summary page.
4. Summary page shows: namespace, tool count and names, schedules and webhooks (count, next fire), calls today vs plan limit, state bytes vs quota, plan name, and an "Upgrade to Pro" link that starts `billing.checkout` with `customer_email` pre-filled.
5. Email sending goes through an HTTP JSON provider configured by `MCPHOST_EMAIL_API_URL`, `MCPHOST_EMAIL_API_KEY`, `MCPHOST_EMAIL_FROM`; a fake provider in tests records sends (same pattern as the fake Stripe client, src/billing.rs:516). When unset, claim pages render but say "email delivery is not configured" and the operator sees `claim_email_configured=false` in admin healthz.
6. Admin healthz adds `tenants_claimed` split external/synthetic and `claims_by_source` keyed by `signup_source` when that column exists (see PRD-mcphost-signup-kill-switch-and-source; absent column means the key is omitted, never an error). `admin.tenants` rows gain `owner_verified: bool` (additive; never the address itself).
7. Rate limits: claim page and verify endpoints are limited per IP (default 30/hour, `MCPHOST_CLAIM_RATE_LIMIT_PER_HOUR`); magic-link sends are limited to 3 per tenant per hour.

P1
8. `host.whoami` reports `owner_verified` so the agent can tell its human whether the claim happened.
9. llms.txt and the signup result include the sentence the agent should relay: "Claim this backend so it belongs to you: <claim_url> (link expires in 7 days)."

P2
10. A second claim attempt on an already-claimed tenant renders "already claimed" without revealing the owner.

## Success metrics

| Metric | Baseline | Target | Method | Timeframe |
|---|---|---|---|---|
| Claimed external tenants / external signups | 0 / 0 | ≥ 50% | admin healthz nightly | 4 weeks after hold lifts |
| Claim page to verified email, median | n/a | < 2 min | timestamps owner_verified_at minus claim page first GET | same |
| Pro checkouts started from the claim page | 0 | ≥ 4 | billing.checkout source field | same |
| Guardrail: magic-link sends per tenant per hour | n/a | ≤ 3 | rate limiter counter | continuous |

## Technical considerations

- Reuse the handoff-token machinery for claim tokens (single-use, TTL, src/control.rs:26-32, 177-198); store claim and verify codes in a new table, never in the tenants row.
- Pages are server-rendered static HTML in www/ style; no JS framework. Never render the bearer key on any page.
- The provider call runs in a spawned task with one retry; failure is journaled and surfaced on the page as "we could not send the email, try again in a minute".
- Coordinate additive schema with PRD-mcphost-admin-schema-contract (queued): new admin fields are optional keys.

## Migration / compatibility

New migration adds `owner_email`, `owner_verified_at`, `claim_token_hash`, `claim_expires_at` (nullable) and a `claim_codes` table. Existing tenants are unclaimed; the operator can mint a claim URL for an existing tenant with `admin.tenant_claim_url`. Signup response is additive; existing clients ignore `claim_url`.

## Open questions

| Question | Owner | Due |
|---|---|---|
| Provider: Resend or Postmark; sending domain and SPF/DKIM | Joe | before build |
| Should a claimed Free tenant get higher quotas than an unclaimed one? | Joe | after first 10 claims |

## Acceptance criteria

1. P0 — Given an unauthenticated `signup` (key mode or handoff mode), When it succeeds, Then the response contains `claim_url` matching `^https://[^/]+/claim/[A-Za-z0-9_-]{40,}$` and the token is not the bearer key.
2. P0 — Given a valid unexpired claim token and a fake email provider, When `POST /claim/{token}` is sent with `email=a@b.co`, Then exactly one provider send is recorded containing a verify URL, and the page body contains "check your inbox".
3. P0 — Given the verify URL from AC2, When it is opened once, Then `tenants.owner_email` is `a@b.co`, `owner_verified_at` is set, the summary page lists the tenant's published tool names and schedule count, and opening the same verify URL again returns 410.
4. P0 — Given an expired claim token (TTL forced to 1 s in test), When `GET /claim/{token}` is requested, Then 410 with "link expired" and no provider send occurs.
5. P0 — Given `POST /claim/{token}` with an empty or malformed email, When submitted, Then 400-class page with the form re-rendered and zero provider sends.
6. P0 — Given `MCPHOST_EMAIL_API_URL` unset, When `GET /claim/{token}` is requested, Then the page renders with "email delivery is not configured" and admin healthz reports `claim_email_configured=false`.
7. P0 — Given two verify requests for the same tenant racing (two different codes issued to two emails), When both complete, Then exactly one owner is set and the loser receives 409.
8. P0 — Given 31 `GET /claim/*` requests from one IP within an hour at the default limit, When the 31st arrives, Then 429 and the tenant row is unchanged.
9. P0 — Given a claimed tenant, When the operator calls admin healthz, Then `tenants_claimed.external` counts it and `admin.tenants` shows `owner_verified: true` without an email address in the row.
10. P0 — Given the fake email provider returns 500 on the first attempt and 200 on the retry, When the magic link is requested, Then one send is ultimately recorded and the page shows "check your inbox".
11. P1 — Given a claimed tenant, When the agent calls `host.whoami`, Then `owner_verified` is true; for an unclaimed tenant it is false.
12. P1 — Given www/llms.txt on the built tree, When grepped, Then it contains the relay sentence with the literal `claim_url` placeholder and "7 days".
13. P0 — Given no page in www/ or any template, When grepped for the bearer key of a test tenant after a full claim flow, Then zero occurrences in any rendered response body.
