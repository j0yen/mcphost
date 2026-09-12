# PRD — mcphost-inbound-events: a tool has a public URL that checks the signature and runs the tool with the event

- Status: built
- Built: 2026-09-12
- Receipts: /home/jsy/wintermute/mcphost/target/autobuilder/receipts (gate: pass=25 block=0 verdict=pass at d1b35ae298b79506ad61827fbf64e0dfec61146d, tag v0.44.0; rollback base v0.43.1 at 342cd50bb5805f4e2540b52b5e71bd0eabdb05e6)
- Lane: redbaron 2026-09-12T04:15:28Z pid=2319999 boot=c6865fd1-71c2-48cf-818e-5e1f2246b3fe
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_priority: high
- build_version_bump: minor
- publish: j0yen/private
- test_prefix: hooks
- Vision: visions/mcp-host.md
- Depends-on: PRD-mcphost-runs-and-jobs.md, PRD-mcphost-schedules.md
- Loop: mcphost-buildloop: satisfaction for integrator-shaped tasks; runs completed per event trigger
- PM: Joe
- Drafted: 2026-09-09
- Engineering target: extend ~/wintermute/mcphost: a `POST /hooks/{namespace}/{tool}` route in `src/http.rs`, the event kind of trigger in the `triggers` table and `host.trigger.*` tools, signature verification using `src/secrets.rs`, runs with `trigger='event'`; the Caddy catch-all in mcphost-deploy already proxies the path

## TL;DR

The most common thing built on every function host is a webhook relay: something posts an event, a small function transforms it, and a notification or a stored row comes out. Val Town puts it on its homepage; n8n's largest non-AI categories are webhook-in chains. On mcphost a tool can only be called by an MCP client with a key, so nothing outside can wake it. The capability panel's integrator segment set its gate plainly: "webhook signature verification built in; no HMAC validation step outside your platform. I paste the secret, you handle it," and "inspect a real webhook payload before it runs anywhere." This PRD gives each tool a public URL, verifies the sender's signature with a secret the tenant already stores, records the event as a run, answers the sender in under a second, and runs the tool as a job with the event as its argument. It is the second kind of trigger on the surface schedules introduced.

## Problem statement

An integrator who wants GitHub, Stripe or a monitoring system to reach a tool has to stand up an endpoint elsewhere and verify signatures by hand, which is the work they came to stop doing. Facts:

- The router (`src/http.rs:297-306`) serves `/mcp`, `/healthz`, `/.well-known/mcp/{namespace}/server.json`, and the billing webhook and pages; every tool call enters through `/mcp` with a key. There is no tenant-facing inbound route.
- The Caddy site template (`mcphost-deploy` `install.py:115-183`) proxies any unmatched path to the backend, so `/hooks/...` needs no deploy change; the `dynamic_zone` rate limit (60 per minute per source) applies to it as written.
- Secrets exist per tenant (`secrets(id, tenant_id, name, value_enc, nonce)`, AES-256-GCM, `src/secrets.rs`) and are resolved at call time; a webhook secret is the same kind of row.
- Panel: integrator_03 "webhook signature verification built in… I paste the secret, you handle it. Done"; integrator_02 "let me define and test webhook handlers locally with full signature verification before hitting production"; integrator_01 in the five-minute test: "Paste a GitHub webhook payload to test." Switch test 1.000. wrapper_01's top pain: "silent failures in stateless API chains… a poller across five Zapier steps stopped mid-run without alerting anyone."
- Open-source: Val Town HTTP and Email vals, n8n Webhook trigger, Zapier webhooks by Zapier; Cloudflare requires hand-rolled WebCrypto verification (no packaged feature found).

The consequence: the integrator segment, which the discover packs of 2026-09-02 and 2026-09-09 both put at the top of switch share, cannot connect anything to mcphost that it does not call itself.

## Goals

1. Every tool can have a public URL; a request to it is verified, recorded, acknowledged fast, and run as a job.
2. Signature verification for the common schemes is a config choice, not code the tenant writes.
3. An event can be inspected and replayed before and after the tool handles it.

## Non-goals

- Outbound webhooks (a tool posting to others); the http kind does that already.
- Email, queue or socket ingress; the trigger model leaves room.
- Custom verification code in the first version; unsupported schemes use `none` with an explicit opt-in.

## User stories

- **Integrator (agent):** `host.secret_set("gh_hook", "<secret>")` then `host.trigger.set(tool="gh_push", kind="event", verify={"scheme": "hmac-sha256", "header": "X-Hub-Signature-256", "secret": "gh_hook", "prefix": "sha256="})` returns `{url: "https://mcphost.dev/hooks/t_ab12/gh_push", trigger_id}`. GitHub posts; the tool runs with `{event: {headers, body, received_unix}}`.
- **Integrator testing:** `host.trigger.test(trigger_id, body=<payload>, headers={...})` verifies and runs it as a job without exposing anything, and returns the run id; a wrong secret returns `signature_invalid` with the header it checked.
- **Agent after an outage:** `host.runs.list(trigger="event", status="error")` then `host.trigger.replay(run_id)` re-runs the stored event.
- **Sender:** gets 202 with `{run_id}` within 500 ms, or 401 for a bad signature, or 429 above the plan's events per minute, or 413 above the body cap.
- **Operator (Joe):** `admin.triggers(kind="event")` and `/healthz` `events_received_1h`, `events_rejected_1h`.

## Requirements

1. P0 — Route `POST /hooks/{namespace}/{tool}`: resolves the tenant by namespace and the tool by name; requires an enabled event trigger on that tool; enforces `MAX_REQUEST_BODY_BYTES`; verifies the signature per the trigger's `verify` config; on success stores the event (headers allowlist plus body, bounded) as a run with `trigger='event'`, `status='queued'`, enqueues it through the executor with args `{event: {...}}`, and answers `202 {run_id}`. Failures answer `401 signature_invalid`, `404 hook_not_found`, `413`, `429 events_rate_limited`, and never run the tool.
2. P0 — Verification schemes: `hmac-sha256` (header name, secret name, optional prefix, optional timestamp header and tolerance for Stripe-style `t=,v1=` values), `hmac-sha1` (GitHub legacy), `token` (a shared token in a named header), `none` (requires `allow_unverified: true` in the config and is labeled so in `host.trigger.list`). Secrets are read through `src/secrets.rs` and never logged.
3. P0 — Event trigger tools: `host.trigger.set(kind="event", tool, verify, args?)` returns the URL; `list/get/pause/resume/remove` shared with schedules; `host.trigger.test(id, body, headers)` and `host.trigger.replay(run_id)`.
4. P0 — Ceilings from the plan: `event_triggers_max` (free 3, pro 25), `events_per_minute` (free 30, pro 300, per trigger), `event_body_bytes_max` (256 KiB); refusals named. `host.quickstart` `limits` lists them.
5. P0 — The stored event is redacted of any header not on the allowlist (`content-type`, `user-agent`, the signature header name, `x-request-id`, and any header the trigger config names) and is subject to the tenant's state quota through the run's result storage rules.
6. P0 — `/healthz` (admin) gains `events_received_1h`, `events_rejected_1h`; `host.usage` counts event runs.
7. P1 — Idempotency: an optional `dedupe_header` in the config (e.g. `X-GitHub-Delivery`); a repeated id within 24 h answers 202 with the original run id and does not run again.
8. P1 — A per-trigger `secret_rotate` flow: two secrets accepted for a grace window named in the config.

## Success metrics

| metric | baseline | target | method | timeframe |
|---|---|---|---|---|
| time to 202 for a valid event | none | p95 ≤ 500 ms at 30 per minute | stress suite webhook storm | first stress run after deploy |
| integrator-shaped corpus tasks completable | 0 | 2 new event tasks pass in fake mode and live (fixture sends the events) | capability-tasks PRD | after both ship |
| guardrail: bad-signature events that run a tool | none | 0 | test | at build |

## Technical considerations

- The route sits beside `/billing/webhook`, which already verifies a Stripe signature; reuse its HMAC helper.
- Rate limiting per trigger is in-process (a token bucket keyed by trigger id); the Caddy zone stays as an outer wall.
- Event bodies are stored on the run row (`args_json`) rather than in the state store, so replay does not depend on state quota; a size cap keeps the row bounded.
- The URL format `https://<public_url>/hooks/<namespace>/<tool>` uses `MCPHOST_PUBLIC_URL`; the namespace is not a secret, the signature is the credential.

## Migration / compatibility

Adds a route and trigger kind; no change to existing paths. The compat check's four steps are unaffected.

## Open questions

| question | owner | due |
|---|---|---|
| Hooks under the main domain behind the existing 60-per-minute Caddy zone, or a second hostname with a hooks zone | Joe | at build |
| Free-plan ceilings | Joe | at build |

## Acceptance criteria

1. P0 — Given an event trigger with `hmac-sha256` over secret `s`, When a POST arrives with a correct `X-Hub-Signature-256`, Then the response is 202 with a `run_id` within 500 ms, and the run finishes with the tool having received `event.body`.
2. P0 — Given the same trigger, When the signature is wrong, Then 401 `signature_invalid`, no run is created, and nothing is logged containing the secret.
3. P0 — Given a Stripe-style `t=,v1=` signature with a timestamp older than the tolerance, When posted, Then 401 with `reason: timestamp`.
4. P0 — Given a trigger with `verify.scheme = "none"` and no `allow_unverified`, When set, Then `trigger_invalid` says unverified triggers need the flag; With the flag, `host.trigger.list` shows `unverified: true`.
5. P0 — Given a free trigger receiving 40 events in a minute, When the 31st arrives, Then 429 `events_rate_limited` and no run.
6. P0 — Given a 300 KiB body, When posted, Then 413 and no run.
7. P0 — Given `host.trigger.test(id, body, headers)` with a valid signature, When called, Then a run is created marked `test: true` and the response carries its id.
8. P0 — Given a failed event run, When `host.trigger.replay(run_id)` is called, Then a new run starts with the same `event` args and `trigger_ref` naming the original.
9. P0 — Given a paused trigger, When an event arrives, Then 404 `hook_not_found` and no run.
10. P0 — Given the admin `/healthz`, When read after AC1 and AC2, Then `events_received_1h` ≥ 1 and `events_rejected_1h` ≥ 1.
11. P1 — Given `dedupe_header: "X-GitHub-Delivery"`, When the same delivery id arrives twice, Then the second answers 202 with the first run id and no second run exists.
