# PRD: mcphost-handoff-token — the credential leaves the transcript

- Status: queued
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_priority: normal
- build_version_bump: minor
- publish: j0yen/private
- test_prefix: handoff
- deferred_acs: [9]
- mock_unjustified_for: [9]
- mock_justifications:
  - "AC9 (a corpus-task probe/task variant exercising signup-via-handoff against the demand/synthorg harness) needs a cross-repo integration with the synthorg measurement harness this PRD's own text names as a fallback ('until then, one proxy-tier harness probe config exercises it') -- a different subsystem than mcphost itself, and P1 by the PRD's own numbering. Every P0 AC (1-7) and the other P1 (AC8, host.whoami key age/rotation) are implemented and tested in this build; AC9's harness-side wiring is deferred rather than faked with a mcphost-side stub that wouldn't actually exercise the corpus harness."
- Vision: visions/mcp-host.md
- Grounding: opportunity — wwhtbt leaf 5 in visions/mcp-host.md; operator pull-forward 2026-09-12 of the cycle-11 deferral
- Loop: mcphost-buildloop: satisfaction
- PM: Joe Yen
- Drafted: 2026-09-12
- Engineering target: extend `~/wintermute/mcphost` — signup flow (`src/handler.rs`), auth resolution (`resolve_auth` and the `tenant_key` argument path), key storage (`src/db.rs`), a rotation surface, docs/llms.txt

## TL;DR

The cycle-11 design decision put the tenant key in the model's context as a tool
argument — accepted deliberately, with "a single-use handoff token recorded as
deferred rather than dismissed." Joe pulled it forward on 2026-09-12 with the
instruction to test it. This PRD ships the token: `signup` returns a short-lived,
single-use handoff token instead of the raw key; one `host.redeem` call exchanges it
for the tenant key and kills the token; a session that leaked its transcript leaks a
dead credential plus whatever window remains on the live key, which `host.key_rotate`
can close. The existing raw-key flow keeps working behind a flag so nothing breaks,
and the harness proves the new path end-to-end.

## Problem statement

Every tenant key ever issued has passed through a model context and, for harness
sessions, into recorded transcripts on disk (42 recordings retained from one run
alone, vision 2026-09-09). The key travels as a `tenant_key` argument on all 17
tenant tools, so it also appears in any client-side logging an agent's harness keeps.
Today's exposure is bounded because every tenant is synthetic — which is exactly why
now is the time to fix it: the moment the first real agent signs up (the event the
demand fleet exists to detect), its credential's exposure story is set. The vision's
own open question — "should a tenant key be passable as a tool argument at all,
given it enters the model's context and any client-side transcript?" — has stood
since cycle 11 with "before the endpoint carries a real tenant" as its due date.

## Goals

- A transcript containing everything a signup session saw contains no credential
  that still works, once the session rotates or the token is redeemed elsewhere.
- The redeem and rotate surfaces are agent-usable within one session (no reconnect,
  no header change — the constraints that forced key-as-argument still hold).
- The change is provably compatible: every existing tool call with a raw
  `tenant_key` argument continues to work unchanged.

## Non-goals

- No OAuth, no session binding (architecturally unavailable: stateless transport,
  `with_legacy_session_mode(false)` — cycle 11's ruling stands).
- No forced migration of existing tenants or the harness (they hold raw keys; they
  keep working; rotation is available, not mandatory).
- No change to admin auth.

## User stories

1. **Agent signing up.** When I sign up, I want a token I redeem once for my key, so
   the signup reply sitting in my context and logs is worthless to anyone who reads
   them later.
2. **Agent that suspects exposure.** When my key may have leaked, I want
   `host.key_rotate` to issue a new key and kill the old one in one call, so
   containment is a tool call, not a support ticket.
3. **Operator.** When I audit the journal, I want redemption and rotation events
   recorded (never the secrets), so credential lifecycle is traceable.

## Requirements

**P0**
1. `signup` returns `{tenant_id, handoff_token, expires_in}` — no raw key — when the
   client requests handoff mode (a signup argument); default behavior without the
   argument is unchanged (raw key), so existing clients and the harness never break.
2. `host.redeem` exchanges a valid handoff token for the tenant key exactly once:
   second redemption fails with a structured error; expiry (short, stated bound)
   fails with a distinct structured error; both are journaled (token id, never
   values).
3. `host.key_rotate` (authenticated as the tenant): issues a new key, invalidates the
   old one immediately, returns the new key once; every other tool call with the old
   key fails as unauthenticated from that point.
4. Storage: tokens are stored hashed with expiry and redeemed-at; keys remain hashed
   as today; neither appears in logs, journals, errors, or `tool_logs`.
5. The quickstart and `get_info` instructions describe the handoff flow as the
   recommended path (redeem, then use the key as arguments), with the raw path
   documented as compatible.
6. Test-what-shipped (Joe: "test it"): an integration test drives the full path as
   two client contexts — context A signs up in handoff mode and redeems; a replay of
   context A's transcript (the token) fails to redeem again; context A rotates; a
   call with the pre-rotation key fails; a call with the new key succeeds.

**P1**
7. `host.whoami` reports key age and last-rotation time, so an agent can audit its
   own credential hygiene.
8. A corpus-task proposal (promote-usecase path) covering signup-via-handoff, so the
   harness measures the flow's friction once adopted; until then, one proxy-tier
   harness probe config exercises it (deploy probe or synthorg bootstrap task
   variant, whichever is cheaper to add without a corpus fingerprint change).

**P2**
9. Optional handoff-by-default per plan or host config, for a future where raw-key
   signup is deprecated.

## Success metrics

| metric | baseline | target | method | timeframe |
|---|---|---|---|---|
| Live credential in a signup transcript | always (raw key) | none after redeem+rotate | handoff ACs | on ship |
| Containment after suspected leak | impossible (no rotation) | one tool call | handoff ACs | on ship |
| Existing raw-key flow | works | byte-identical behavior | regression AC | on ship |

## Technical considerations

- The key-as-argument constraint set is unchanged (one connection, static headers);
  handoff+redeem happens inside the same session — the token narrows the exposure
  window, rotation closes it; this is exposure reduction, not elimination, and the
  docs say so plainly.
- Token entropy/length and expiry bound stated in code constants with names; journal
  events follow the existing audit-row conventions (names and ids, never values).
- `resolve_auth` gains rotation-aware key lookup; the hot path must not add a query
  per call (key hash lookup already exists — rotation just replaces the row).

## Migration / compatibility

Additive migration for the token table and rotation columns. Existing tenants, the
harness, and the fleet adapter are untouched (raw path default). Rollback: drop the
new tools; existing keys unaffected.

## Open questions

| question | owner | due |
|---|---|---|
| Handoff token expiry bound (minutes vs the session's practical length) | build | at build, stated in receipt |
| When handoff mode becomes the documented default for real signups | Joe | after first real tenant |

## Acceptance criteria

1. P0 — Given signup with handoff mode, When it completes, Then the response carries a token and expiry and no key, and the journal records issuance without the token value.
2. P0 — Given a valid token, When `host.redeem` is called, Then the tenant key returns exactly once; a second redeem fails with the structured already-redeemed error; an expired token fails with the structured expired error.
3. P0 — Given a redeemed session, When `host.key_rotate` is called, Then a new key returns, the old key fails on the next tool call as unauthenticated, and the new key succeeds.
4. P0 — Given the full two-context integration test (sign up handoff, redeem, rotate, replay old token, replay old key), When it runs, Then every replayed credential from the transcript fails and every live-path call succeeds.
5. P0 — Given signup without the handoff argument, When it completes, Then the response is byte-compatible with today's raw-key response and all 17 tenant tools work unchanged with that key.
6. P0 — Given logs, journal, and tool_logs after the full flow, When grepped for the token and both keys, Then none appear in plaintext anywhere.
7. P0 — Given quickstart, get_info, and llms.txt after build, When read, Then the handoff flow is documented as recommended with the raw path noted compatible.
8. P1 — Given a rotated tenant, When `host.whoami` is called, Then key age and last-rotation time are reported.
9. P1 — Given the probe or task variant exercising handoff, When it runs against a local instance, Then signup→redeem→publish→call completes and is scored/recorded like the raw path.
