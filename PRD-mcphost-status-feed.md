# PRD: mcphost-status-feed — the status page reports what the host measured

- Status: queued
- Lane: orch 2026-09-25T23:50:21.998328189+00:00 run=234
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- test_prefix: statusfeed
- publish: j0yen/private
- Vision: visions/mcphost-operability-floor.md
- Depends-on: PRD-mcphost-alerting-webhook.md
- Grounding: wwhtbt leaf 5 (weakest link) — visions/mcphost-operability-floor.md; readiness gate 6 status page; static `www/status.html`, healthz route http.rs:523 with no history
- PM: Joe
- Drafted: 2026-09-25
- deferred_acs: [11]
- mock_justifications: AC11 -- prod-only proof ("Given prod after land ... `https://mcphost.dev/status.json` is fetched ... `status.html` renders it"). It can only be satisfied by a live deploy of this branch to the real https://mcphost.dev host; deploying is an operator action mcphost-deploy performs, not something a build agent does on its own, and prod cannot serve this branch's `/status.json` shape until that deploy happens, so no run in this worktree can execute the prod leg. The live check is written and ready for the operator's post-ship run, gated on MCPHOST_LIVE=1: tests/statusfeed_ac11_live_status_trailer.rs::status_json_is_operational_with_four_components (plus ::status_html_has_the_render_mechanism_and_is_served_live), which print the fetched `/status.json` body and the live `status.html` byte count for the trailer. The same tests run the identical `/status.json` fetch and `status.html` render-mechanism checks against a real local mcphost server on every cargo test -- that proves the branch's mechanism, not AC11, and is not counted as AC11's proof.
- Engineering target: j0yen/mcphost (`src/http.rs`, `src/cron.rs`, `src/admin.rs`, `src/db.rs`, `www/status.html`)

## TL;DR

`GET /status.json` serves the host's own availability history: per-minute healthz
samples rolled up to 90 days of daily uptime per component (MCP endpoint, tool
execution, billing webhook, claim flow), open and past incidents the operator posts
with `admin.incident.{open,update,close}`, and the current state. `www/status.html`
renders that JSON client-side. No third-party status account is needed to close
readiness gate 6.

## Problem statement

Readiness gate 6 is "Instatus status page" and belongs to Joe's to-do list along with
the email provider and DMCA registration (wiki 2026-09-23-mcphost-market-test-plan;
memory project_mcphost_discovery_synthesis_20260922). Today `www/status.html` is a
static file the deploy uploads and nothing updates; the host records healthz results
nowhere (scout 2026-09-25: healthz is a route in http.rs:523 with no history table).
A stranger who hits an error during a launch has no page that says whether the host
knows; the operator has no history to answer "was it down last night". Each day the
gate stays open is a day the hold cannot lift for a reason that is one table and one
route away.

## Goals

- A machine-readable status feed the host computes from its own probes.
- Operator-posted incidents with a timeline, through admin tools only.
- The existing static page becomes a renderer of the feed.

## Non-goals

- Email or RSS subscriptions to incidents.
- Probing from outside the box (an external probe belongs to uptime-probes or
  mcphost-deploy; this feed records the host's self-view and any results
  mcphost-deploy posts in).
- Replacing healthz.

## User stories

- **Stranger's agent:** reads `/status.json` and sees `state: "degraded"` with an
  incident title before deciding to retry.
- **Operator:** `admin.incident.open {title, components, impact}` publishes an incident
  in one call; `update` adds a timeline entry; `close` sets resolution.
- **Operator:** 90-day uptime per component is one number per day I can cite in the
  launch post.
- **mcphost-deploy:** posts an external probe result into the feed via
  `admin.status.sample` so the page reflects outside-in reachability too.

## Requirements

**P0**
1. Table `status_samples(component, ts, ok, latency_ms, source)` (migration 0037) and
   `incidents(id, title, impact, components_json, opened_at, closed_at, timeline_json)`;
   samples older than 90 days are pruned daily; a daily rollup table
   `status_daily(component, day, ok_samples, total_samples, p95_latency_ms)`.
2. Every minute the cron loop samples four components from inside the process: `mcp`
   (an in-process `initialize` + `tools/list`), `exec` (an echo-kind tool call through
   the sandbox path), `billing` (webhook route answers a signed no-op), `claim`
   (`/claim/<invalid-token>` answers 404 within budget). Each sample records ok and
   latency with `source: "self"`.
3. `GET /status.json` (anonymous, cacheable 60 s) returns `{state, generated_at,
   components: [{name, state, uptime_90d, uptime_30d, last_sample}], incidents_open:
   [...], incidents_recent_30d: [...]}`; `state` is `operational` when every component's
   last 5 samples are ok and no open incident has impact ≥ `partial`, `degraded` for
   partial impact or one failing component, `outage` for `major` impact or all
   components failing.
4. Admin tools `admin.incident.open {title, impact: minor|partial|major, components}`,
   `admin.incident.update {id, message, state?}`, `admin.incident.close {id, message}`,
   `admin.status.sample {component, ok, latency_ms, source}`; each writes `admin_audit`.
5. `www/status.html` fetches `/status.json` and renders state, components with 90-day
   bars, and incidents; the static fallback text remains when fetch fails.

**P1**
6. When a component fails 5 consecutive self samples and no open incident names it,
   raise alert key `status.component_down` (alerting registry, present per Depends-on)
   and auto-open an incident with `impact: partial` titled from the component; close it
   automatically after 10 consecutive ok samples with a timeline note.
7. `GET /status.json?component=<name>&days=<n>` returns daily rows for one component.

**P2**
8. `admin.status.rollup` recomputes `status_daily` for a day range.

Non-functional: the self-sampler adds ≤ 4 requests per minute of in-process load;
`/status.json` answers from the rollup in < 20 ms.

## Success metrics

| metric | baseline | target | method | timeframe |
|---|---|---|---|---|
| readiness gate 6 | open (vendor account pending) | closed by feed | wiki gate table | at land |
| time from component failure to public status change | never | ≤ 5 min | sample timestamps | first incident |
| guardrail: added load | 0 | ≤ 4 in-process calls/min | sampler counter | at gate |

## Technical considerations

- The self samples call the same handler functions the HTTP layer dispatches to, in
  process, with a system tenant; they must not count in `calls` or metering.
- `status.html` already exists and is mirrored to `/var/www/mcphost/` by mcphost-deploy
  on publish (memory project_mcphost_prod_box); the JSON route is served by the host
  behind Caddy like healthz, so the page's fetch path is same-origin.
- Uptime is `ok_samples / total_samples` over the window; missing minutes (host down)
  count as not-ok when a later sample exists that day.

## Migration / compatibility

Two additive tables, four admin tools, one anonymous route, one static page change.
Caddy needs no change (same origin as healthz).

## Open questions

| question | owner | due |
|---|---|---|
| Whether `/status.json` also lists the registry listing state (`dev.mcphost/mcphost`) | Joe | at build |
| Instatus stays as a mirror or is dropped from the gate table once this lands | Joe | before hold lift |

## Acceptance criteria

1. P0 — Given the host has run for 3 minutes, When `GET /status.json` is called anonymously, Then it returns `state: "operational"`, four components each with a `last_sample` within 90 s, and `Cache-Control: max-age=60`.
2. P0 — Given the sandbox executor is disabled in the test harness, When 5 sample ticks pass, Then `components[exec].state` is `failing` and `state` is `degraded`.
3. P0 — Given `admin.incident.open {title, impact: "major", components: ["mcp"]}`, When `/status.json` is called, Then `state` is `outage`, the incident appears in `incidents_open`, and `admin_audit` holds the open entry.
4. P0 — Given an open incident, When `admin.incident.update {id, message}` then `admin.incident.close {id, message}` run, Then the timeline has three entries in order and the incident moves to `incidents_recent_30d`.
5. P0 — Given 1 440 samples for one day with 1 425 ok, When the daily rollup runs, Then `status_daily` shows `ok_samples: 1425` and `/status.json` reports `uptime_30d` computed from the rollup.
6. P0 — Given `admin.status.sample {component: "mcp", ok: false, latency_ms: 0, source: "deploy-probe"}`, When posted 5 times, Then the component's state reflects the external samples and `source` is preserved per row.
7. P0 — Given samples older than 90 days, When the daily prune runs, Then they are deleted and the rollup rows for those days remain.
8. P0 — Given `www/status.html` is served, When the page loads with `/status.json` reachable, Then it renders the four components and incident list; when the fetch fails, the static fallback text is visible.
9. P1 — Given the alerting registry is present and a component fails 5 consecutive samples, When the tick runs, Then one `status.component_down` alert is raised and an incident with `impact: partial` is open; after 10 ok samples it is closed with a timeline note.
10. P1 — Given `GET /status.json?component=mcp&days=7`, When called, Then seven daily rows with `ok_samples`, `total_samples`, `p95_latency_ms` return.
11. P0 — Given prod after land, When `https://mcphost.dev/status.json` is fetched, Then it returns `operational` with four components and `status.html` renders it (Live; evidence: `mcphost-1: healthz version after deploy plus the call transcript this AC names, saved under docs/receipts/<slug>.md`)
