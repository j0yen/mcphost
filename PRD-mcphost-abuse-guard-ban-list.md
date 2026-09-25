# PRD: mcphost-abuse-guard-ban-list — stop an abuser in one call, without a redeploy

- Status: queued
- Lane: orch 2026-09-25T08:37:12.340554625+00:00 run=186
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- test_prefix: banlist
- publish: j0yen/private
- Vision: visions/mcphost-operability-floor.md
- Grounding: wwhtbt leaf 4 — visions/mcphost-operability-floor.md; readiness "captcha/ban list absent"; evidence tables `network_denials` (0033), `claim_rate_events` (0034), `state.signup_pause` (control.rs:119)
- PM: Joe
- Drafted: 2026-09-25
- Engineering target: j0yen/mcphost (`src/control.rs`, `src/admin.rs`, `src/auth.rs`, `src/network_policy.rs`, `src/db.rs`)

## TL;DR

A ban list keyed by tenant key, signup address, or claim email domain refuses signups,
tool calls, and claims for banned subjects with a stable error; operators add and
remove entries with `admin.ban.{add,remove,list}`; the host adds temporary entries on
its own when one subject accumulates egress denials or claim-rate events past a
threshold; every entry carries a reason, an expiry, and an audit row.

## Problem statement

The readiness plan lists "captcha/ban list" as absent (wiki
2026-09-23-mcphost-market-test-plan). The host already records the evidence of abuse —
`network_denials` (migration 0033) for tenants whose tools try to reach forbidden
destinations, `claim_rate_events` (0034) for claim-code hammering, `signup_events` with
source and address — but the only response available to the operator is the global
signup pause (control.rs:119), which stops every stranger to stop one. During a Show HN
wave that is a self-inflicted outage. Without a ban list the free-compute abuse case the
egress allowlist was built to contain (vision market-test leaf 3) has no per-subject
remedy, and the operator's only tool is to redeploy or hand-edit the database.

## Goals

- One admin call bans a subject; the ban is effective on the next request.
- Automatic temporary bans on the two abuse signals the host already records.
- Every ban is auditable and expires unless made permanent.

## Non-goals

- CAPTCHA or proof-of-work on signup (a separate decision; the market-test plan
  pairs it with this list but it changes the signup contract).
- IP reputation feeds or ASN blocking.
- Banning by email address of a claimed human beyond domain (privacy scope).

## User stories

- **Operator:** an agent is hammering `signup` from one address; `admin.ban.add
  {subject: "addr:203.0.113.9", ttl: "24h", reason: "signup flood"}` stops it while other
  strangers keep signing up.
- **Operator:** a tenant's tool keeps hitting the cloud metadata address; the host has
  already banned the key for an hour and told me via alert.
- **Operator:** `admin.ban.list` shows what is banned, why, until when, and who did it.
- **Banned agent:** receives a structured error naming the ban and, for temporary bans,
  the expiry, so it does not retry in a loop.

## Requirements

**P0**
1. Table `bans(id, subject_kind, subject, reason, created_at, created_by, expires_at,
   auto, hits)` (migration 0036); `subject_kind ∈ {key, addr, email_domain}`.
2. Enforcement points: `signup` (addr, email_domain on claim), every authenticated tool
   call (key), `/claim/*` routes (addr, email_domain), `/hooks/*` inbound (key of the
   owning tenant). A banned subject receives error code `banned` with `reason` (only
   when the operator marked it `public: true`), `expires_at`, and no other detail;
   `hits` increments.
3. Admin tools: `admin.ban.add {subject_kind, subject, ttl|permanent, reason, public}`,
   `admin.ban.remove {id}`, `admin.ban.list {active_only, subject_kind}`; each writes an
   `admin_audit` entry. `ttl` accepts `30m`, `24h`, `7d`; permanent requires the literal
   `"permanent": true`.
4. Automatic bans: (a) a tenant key with ≥ `MCPHOST_BAN_DENIALS_THRESHOLD` (default 25)
   `network_denials` rows in 10 min gets a 1 h auto ban; (b) an address with ≥
   `MCPHOST_BAN_CLAIM_RATE_THRESHOLD` (default 30) `claim_rate_events` in 10 min gets a
   1 h auto ban; auto bans set `auto=true`, reason names the rule and count, and raise
   alert key `ban.applied` when the alerting registry is present.
5. Expired bans stop enforcing at expiry without a restart; a sweep every 60 s in the
   cron loop deletes rows expired for more than 7 days, keeping the audit row.

**P1**
6. An in-memory cache of active bans refreshed on write and every 30 s, so enforcement
   costs one hash lookup per request.
7. healthz (operator header) includes `bans: {active, auto_active, hits_24h}`.

**P2**
8. `admin.ban.add` accepts a CIDR for `addr` (v4 /24 minimum, v6 /48 minimum).

Non-functional: enforcement adds < 50 µs per request; a ban list of 10k entries fits
the cache.

## Success metrics

| metric | baseline | target | method | timeframe |
|---|---|---|---|---|
| time to stop one abusive subject | redeploy or global pause (minutes, all users) | < 10 s, one subject | admin call to first refused request | at gate |
| strangers blocked by a global pause during abuse | all | 0 | pause not needed for single-subject abuse | first incident |
| guardrail: false auto-bans | n/a | 0 in the load proof | showhn profile run | at load proof |

## Technical considerations

- `network_denials` and `claim_rate_events` already carry the subject and timestamp;
  the auto-ban rule is a windowed count query on the minute tick, not a per-event hook.
- The `banned` error joins the existing structured error set in `errors.rs`; the
  signup path returns it before `signup_events` is written, so bans do not inflate
  attribution.
- The claim routes read the requesting address from the same header Caddy already
  forwards for rate limiting.

## Migration / compatibility

Additive migration, three admin tools, two thresholds as env vars. No client-visible
change for non-banned subjects.

## Open questions

| question | owner | due |
|---|---|---|
| Auto-ban durations (drafted 1 h) and whether a second auto-ban within 24 h escalates to 24 h | Joe | at build |
| Is `reason` ever public by default (drafted: never unless `public: true`) | Joe | at build |

## Acceptance criteria

1. P0 — Given `admin.ban.add {subject_kind: "addr", subject: "203.0.113.9", ttl: "24h", reason: "flood"}`, When `signup` is called from that address, Then the response is error `banned` with `expires_at` set and no `signup_events` row is written; from another address signup succeeds.
2. P0 — Given a banned tenant key, When any authenticated tool is called with it, Then error `banned` returns, `hits` increments, and the call is not recorded in `calls`.
3. P0 — Given a ban with `public: false`, When the banned subject is refused, Then the error carries no `reason`; with `public: true` it carries the reason string.
4. P0 — Given a tenant key with 25 `network_denials` rows in the last 10 min, When the minute tick runs, Then a ban row with `auto=true` and a 1 h expiry exists for that key and, if the alerting registry is present, one `ban.applied` alert is raised.
5. P0 — Given an address with 30 `claim_rate_events` in 10 min, When the tick runs, Then an auto ban exists for `addr` and `/claim/verify/<code>` from that address returns `banned`.
6. P0 — Given a ban whose `expires_at` is 2 s away, When 3 s pass and the subject calls, Then the call succeeds without a restart.
7. P0 — Given `admin.ban.remove {id}`, When the subject calls, Then it succeeds and `admin_audit` holds both the add and the remove entries with the operator identity.
8. P0 — Given `admin.ban.list {active_only: true}`, When called after two active and one expired ban, Then two rows return with `hits`, `reason`, `expires_at`, `auto`.
9. P0 — Given a permanent ban request without the literal `permanent: true`, When `admin.ban.add` is called with neither `ttl` nor `permanent`, Then it is rejected with a validation error.
10. P1 — Given 10 000 active bans, When 1 000 requests run, Then p99 added latency from enforcement is < 1 ms (in-memory cache).
11. P1 — Given the operator healthz header, When `GET /healthz` is called, Then `bans.active`, `bans.auto_active`, and `bans.hits_24h` are present.
12. P0 — Given prod after land (Live), When the operator bans and unbans a test address on mcphost-1, Then a signup from that address is refused while banned and accepted after removal, and both actions appear in `admin.ban.list` history.
