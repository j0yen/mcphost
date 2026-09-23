# PRD: mcphost-signup-kill-switch-and-source — pause signups without a restart; attribute every signup to a channel

- Status: queued
- Lane: orch 2026-09-23T06:23:16.963856793+00:00 run=146
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_priority: high
- publish: j0yen/private
- Vision: visions/mcphost-market-test.md
- Cited-tree: mcphost@609f122903757fd85ee23e6f6c6bba4135b59344
- Loop: mcphost-buildloop: satisfaction — external signups by source
- PM: Joe
- Drafted: 2026-09-22
- Engineering target: mcphost (src/control.rs, src/state.rs, src/http.rs, src/db.rs, www/llms.txt)

## TL;DR

`signup` accepts an optional `source` string that is stored on the tenant and broken out in admin healthz and `admin.tenants`. A pause file named by `MCPHOST_SIGNUP_PAUSE_FILE` stops new signups within one second of being created and resumes when removed, no restart, with a structured `signup_paused` error carrying an operator message.

## Problem statement

The launch plan runs three traffic waves three days apart and needs to know which one produced each tenant; today `signup(attribution, source_ip, ...)` (src/control.rs:86-121) records only provenance class (synthetic/external) and origin, so every stranger looks the same. The only abuse brake is the per-IP limit (`MCPHOST_SIGNUP_RATE_LIMIT_PER_HOUR`, src/state.rs:32-35) read at startup; there is no way to stop signups during a bad hour except a redeploy through mcphost-deploy, which the operator wants to avoid at 2 am on launch night (plan §2 item 3). Railway and Fly both lost their free tiers to exactly this kind of unbraked abuse.

## Goals

- Every external signup carries a channel label the operator chose in the link or snippet.
- Signups can be paused and resumed in under a second by a file touch.

## Non-goals

- Referrer capture on www pages (separate, static-site concern).
- Per-source quotas or pricing.
- Captcha.

## User stories

- Operator: I post the HN link with `source: "hn"` in the snippet and the Reddit link with `source: "reddit"`; the next morning healthz tells me signups per source.
- Operator: mining bots start signing up at 3 am; I `touch` the pause file over ssh and signups stop; existing tenants keep running.
- Agent: I call signup during a pause and get a clear error with when to retry, not a 500.
- Measure loop: the nightly tenant read includes `signup_source` so the market-test dashboard needs no new query.

## Requirements

P0
1. `signup` accepts optional `source` (string, 1–64 chars, `^[a-z0-9][a-z0-9._-]*$`); invalid values are rejected with `invalid_argument` naming the field; absent means `signup_source = NULL`.
2. `tenants.signup_source` column (nullable, indexed) written at signup; `admin.tenants` rows include `signup_source` (additive); admin healthz adds `signups_by_source` (external only) for the last 24 h and all-time.
3. `MCPHOST_SIGNUP_PAUSE_FILE` (path; default `<data_dir>/signup.paused`): when the file exists, `signup` returns error code `signup_paused` with `message` from the file's first line (or a default) and `retry_after_secs` (default 3600); the check is per request (stat), never cached beyond 1 s.
4. Paused state never affects authenticated calls, tool runs, triggers, or billing.
5. Admin healthz reports `signups_enabled: bool` and `signup_pause_message` when paused.

P1
6. www/llms.txt and README document the `source` argument with the recommended values `hn`, `reddit`, `discord`, `registry`, `plugin`, `docs`.
7. The pause file may contain a second line `until=<unix_ts>`; when present, `retry_after_secs` is derived from it.

P2
8. `admin.signup_pause` tool that creates/removes the file with a message, for operators without shell access.

## Success metrics

| Metric | Baseline | Target | Method | Timeframe |
|---|---|---|---|---|
| External signups with non-null source | 0% | ≥ 80% | admin healthz | first 2 weeks after hold lifts |
| Pause-to-first-refusal latency | n/a (redeploy, minutes) | < 1 s | test AC3 timing | at build |
| Guardrail: paused signups affecting tool runs | n/a | 0 | AC4 | at build |

## Technical considerations

- Keep `source` separate from `origin`/`origin_detail` (src/control.rs:125); provenance is computed, source is claimed by the caller and therefore untrusted (display only).
- File flag rather than env so mcphost-deploy's env contract is untouched; document the path in README ops section.
- Rate limiter untouched (PRD-mcphost-signup-rate-configurable shipped the env knob).

## Migration / compatibility

One additive migration (`signup_source TEXT NULL` + index). Existing tenants stay NULL. Signup response unchanged apart from echoing `source`.

## Open questions

| Question | Owner | Due |
|---|---|---|
| Should synthetic (synthorg) signups also carry source for harness runs? Draft says yes, labelled `synthorg` | Joe | at build |

## Acceptance criteria

1. P0 — Given `signup(name, source: "hn")`, When it succeeds, Then the response echoes `source: "hn"` and the tenant row has `signup_source = 'hn'`.
2. P0 — Given `signup(name)` with no source, When it succeeds, Then `signup_source` is NULL and the response omits or nulls `source`.
3. P0 — Given `source: "HN!"` or a 65-character value, When signup is called, Then `invalid_argument` names `source` and no tenant is created.
4. P0 — Given the pause file is created while the server runs, When `signup` is called 1 s later, Then the error code is `signup_paused`, `message` equals the file's first line, and `retry_after_secs` is present; When the file is removed, Then a signup 1 s later succeeds. No restart in between.
5. P0 — Given the pause file exists, When an existing tenant calls `host.tool_call`, `host.trigger.fire`, and `billing.status`, Then all succeed as before.
6. P0 — Given three external signups with sources `hn`, `hn`, `reddit` and one synthetic signup, When admin healthz is read, Then `signups_by_source.external` is `{"hn": 2, "reddit": 1}` and `signups_enabled` is true.
7. P0 — Given the pause file exists, When admin healthz is read, Then `signups_enabled` is false and `signup_pause_message` matches the file.
8. P0 — Given 50 concurrent signups while the pause file is toggled on mid-burst, When the burst completes, Then every response is either success or `signup_paused`, never a 500, and tenant count equals the success count.
9. P1 — Given www/llms.txt on the built tree, When grepped, Then it documents `source` and lists `hn` and `registry`.
10. P1 — Given a pause file whose second line is `until=<now+120>`, When signup is called, Then `retry_after_secs` is between 100 and 120.
