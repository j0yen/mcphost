# PRD: mcphost-tenant-data-export — a tenant can take its things when it leaves

- Status: queued
- Lane: orch 2026-09-20T08:05:17.182378708+00:00 run=32
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_priority: low
- publish: j0yen/private
- Vision: visions/mcp-host.md
- Loop: grand-loop: paid_mrr_usd — retention guardrail: portability is the trust signal a paying tenant checks before the plan limit
- Grounding: wwhtbt — visions/mcp-host.md addendum 2026-09-18, leaf 6: v0.57.0 ships self-offboard (delete) and there is no `host.export`; the pair of "leave the way you joined" is "take what you made"
- PM: Joe
- Drafted: 2026-09-18
- Engineering target: mcphost `src/handler.rs` (new `host.export`), `src/runs.rs` (export as a job), `src/tenant_state.rs`, `docs/`

## TL;DR

Self-offboard deletes a tenant. Nothing lets the tenant first take its tool sources,
state, secrets' names (never values), runs, and threads. `host.export` starts a job
that writes a single archive the tenant downloads by signed URL for 24 hours, with a
manifest an agent can re-publish from.

## Problem statement

An agent's human evaluates a host by the exit as much as the entry. Evidence:
PRD-mcphost-tenant-self-offboard (0.57.0) covers deletion; `grep -rn export src`
finds no tenant-facing export; the tenant's own artifacts live across `tools`,
`tool_versions` (pending), `state`, `runs`, `threads`. Consequence: the only way out
with data is transcript archaeology, and a "can I leave" question gets a "no".

## Goals

- One call, one archive, everything the tenant made.
- Re-publishable: the manifest maps to `host.tool_publish` arguments.

## Non-goals

- Importing an archive into another tenant (a later PRD).
- Exporting secret values.

## User stories

- As an agent, `host.export` returns a run id; `host.runs` shows it done with a URL.
- As an agent, the archive's `manifest.json` lists each tool with its publish args.
- As the operator, exports count against the plan's job quota and expire in 24 h.

## Requirements

P0
1. `host.export {}` creates a run (existing runs ledger) that writes a `.tar.gz`
   containing `manifest.json`, `tools/<name>/v<N>/source` and `config.json` for the
   current version (all kept versions once tool-versions ships), `state/<key>.json`,
   `runs.jsonl` (metadata, not results over 1 MB), `threads.jsonl`, `secrets.txt`
   (names only), `usage.json`.
2. The archive is stored under the tenant's scratch quota and served by a signed URL
   valid 24 h; the run's result carries the URL and size.
3. The export runs under the tenant's job quota and rlimits; a second export while
   one is running returns the running run id.

P1
4. `manifest.json` entries are valid `host.tool_publish` argument objects (validated
   by the publish schema in a test).
5. `host.export {tools: ["a", "b"]}` exports a subset.

P2
6. `admin.usage` counts exports per day.

## Success metrics

| metric | baseline | target | method | timeframe |
|---|---|---|---|---|
| tenants that can retrieve their tools without support | 0 | 100 % | export test | at ship |
| synthetic task "export and re-publish elsewhere" satisfaction | no task | ≥ 0.9 | synthorg corpus task | first run after ship |

## Technical considerations

- Reuse the runs executor (`runs.rs`) and the scratch quota (`tenant_state.rs`).
- Signed URL: HMAC over path+expiry with the existing session-key material; no new secret.

## Migration / compatibility

None; additive.

## Open questions

| question | owner | due |
|---|---|---|
| Archive size cap per plan (assumed: `free` 50 MB, `pro` 1 GB) | Joe | at build |

## Acceptance criteria

1. P0 — Given a tenant with two tools, three state keys, and one secret, When `host.export` runs to completion, Then the archive contains both tool sources, three state files, `secrets.txt` with one name and no value, and `manifest.json`.
2. P0 — Given a completed export, When the signed URL is fetched within 24 h, Then the archive downloads; after 24 h, Then 410.
3. P0 — Given an export in progress, When `host.export` is called again, Then the same run id is returned.
4. P0 — Given prod after ship, When the homeward tenant runs `host.export` during the build's live check, Then the archive lists its tool and downloads (proof: the run result in the trailer).
5. P1 — Given the manifest, When each entry is validated against the `host.tool_publish` input schema, Then all validate.
6. P1 — Given `tools: ["a"]`, When exported, Then only tool `a` is in the archive.
7. P2 — Given `admin.usage`, When read, Then `exports_today` is present.
