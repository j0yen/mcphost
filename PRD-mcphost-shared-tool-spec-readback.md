# PRD: mcphost-shared-tool-spec-readback — an owner can expose a shared tool's spec so a sharee can read its source

- Status: queued
- Lane: orch 2026-09-26T06:14:08.482169811+00:00 run=253
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- Cited-tree: mcphost@3fd82148
- publish: j0yen/private
- Vision: visions/synthorg-compete.md
- Grounding: wwhtbt leaf 4 + open question "fork visibility" DECIDED source-visible (Joe 2026-09-26) — visions/synthorg-compete.md; origin/main control.rs:1067 tool_list returns name/kind/visibility/env/share_description and never `spec`; db.rs:437 ToolRow.spec holds the python source; no sharee-side read path exists
- Loop: mcphost-buildloop: round_delta
- PM: Joe
- Drafted: 2026-09-25
- deferred_acs: []
- mock_justifications: none. AC10 was previously deferred (prod leg only); it is now paired hermetically by tests/mcphost_shared_tool_spec_readback_ac10_two_tenant_spec_read_and_calls_listing.rs, which drives the whole two-actor sequence -- publish-with-env, group-share with expose_spec, sharee spec read, owner calls-listing readback (host.usage's calls_by_others, since this control plane has no host.calls.list) -- against a real local server on every cargo test. Reachability of this branch at https://mcphost.dev after deploy is verified by the daemon's live stage, not by a build-time test or a hand-taken prod snapshot.
- iter_log: 2026-09-26T06:55Z amended by /dream (Joe decision 2026-09-26) — expose_spec free on every plan; open question closed; no AC changed
- Engineering target: j0yen/mcphost (`src/handler.rs` tool registry + schemas, `src/control.rs` tool_share/tool_list, `src/db.rs` ToolRow, `src/kinds/python.rs` spec redaction, `docs/agent-quickstart.md`, `www/llms.txt`)

## TL;DR

`host.tool_share` gains `expose_spec: bool` (default false). When true, any tenant the tool is shared with can call `host.tool_spec_shared(tool: "<owner_ns>.<name>")` and receive the tool's `kind` and a redacted `spec`: the python source, `args_schema`, `requirements`, `timeout_s`, and `network`, with `env` and every secret reference removed. `host.tool_list` shows `expose_spec` on the owner's side, and the sharee's view of a shared tool says whether its spec is readable. An agent can now build on another agent's published tool the way a developer builds on open source: by reading it.

## Problem statement

WHO: an agent that has been given access to another tenant's tool and wants to improve on it, and, immediately, the compete tournament's round-two builders, who are told to fork round-one winners (the compete rounds PRD, requirement 5). WHAT: read the tool's spec, which for `kind: python` is the source. WHY NOT TODAY: `tool_list` (`src/control.rs:1067`, origin/main) returns `name`, `kind`, `visibility`, `env`, `share_description`, `created_at`, `unshared_by`, and never `spec`; `ToolRow.spec` (`src/db.rs:437`) is written at publish and read only by the call path; a sharee has `host.tool_call` on a `<owner_ns>.<tool>` path (queued PRD-mcphost-shared-tool-call-path) and nothing else. CONSEQUENCE: a "fork" on mcphost is a rewrite from a one-line `share_description`, so the tournament cannot measure recombination, and the product's agent-to-agent story stops at calling, never at building on.

## Goals

- Owner opt-in per share, default closed.
- A sharee reads kind and redacted spec in one call.
- `env` and secret references never leave the owner's tenant.
- Docs and llms.txt show the flow in the same place they show `host.tool_share`.

## Non-Goals

- Exposing spec on public (`visibility: public`) tools to tenants they are not shared with; public visibility and spec exposure stay separate flags.
- Versioning or diffing specs.
- Any change to `host.tool_call` routing (PRD-mcphost-shared-tool-call-path owns that).
- A control plane beyond the share flag itself (single flag, per share, is the control plane).

## User stories

1. As an owner agent, I share `nightly-scrape` to the `arena-builders` group with `expose_spec: true` because I want forks.
2. As an owner agent, I share a tool that reads an API key from `env` and, even with `expose_spec: true`, the sharee sees the source and the env variable *names* but no values or secret refs.
3. As a sharee agent, I call `host.tool_spec_shared` on a tool shared without exposure and get a refusal that says the owner has not exposed it, not a not-found.
4. As a sharee agent, I list what is shared with me and see `spec_readable: true|false` per tool before I try.
5. As the compete rounds runner, I share winners with `expose_spec: true` and record which builders read the spec.

## Requirements

P0
1. `host.tool_share` input schema (`src/handler.rs:846` region) gains `expose_spec` (boolean, default false); the value is stored on the share (new column on the tool row or the share row, whichever the existing share model uses) and returned by `host.tool_list` for the owner as `expose_spec`.
2. New tool `host.tool_spec_shared` with input `tool: "<owner_ns>.<name>"`; the caller must be a member of the group (or the direct sharee) the tool is shared with; response is `{ "tool", "kind", "spec", "exposed_at" }`.
3. Redaction: the returned `spec` omits `env` entirely and any spec field whose value matches the tenant-secret reference form used by `build_secret_resolver` (`src/kinds/python.rs`, secrets section); a test publishes a tool with two env entries and one secret ref and asserts none appear in the sharee's response while `source`, `args_schema`, `requirements`, `timeout_s`, `network` do.
4. Refusals: not shared with the caller → the same not-found shape `host.tool_call` uses for unshared tools (no leak that the tool exists); shared but `expose_spec: false` → `spec_not_exposed` with the owner namespace and tool name; both are logged like other refused calls.
5. `host.tool_unshare` clears exposure; re-sharing without the flag returns to closed.
6. `docs/agent-quickstart.md` and `www/llms.txt` document `expose_spec` and `host.tool_spec_shared` beside `host.tool_share`, with one worked example.
7. Rate limit: `host.tool_spec_shared` counts against the caller's existing per-tenant call quota; no new quota.

P1
8. The sharee's listing of tools shared with it (wherever that appears today; if only via `tool_call` errors, then in the `spec_not_exposed`/not-found response) carries `spec_readable`.
9. `exposed_at` and a `spec_reads` counter on the owner's `tool_list` row so an owner can see the tool is being read.

P2
10. `expose_spec` on public tools for any tenant (separate decision; off by default).

Non-functional: p95 for `host.tool_spec_shared` under 100 ms for a 64 KiB spec; redaction is applied on read, never by mutating the stored spec.

## Success metrics

| metric | kind | baseline | target | method | timeframe |
|---|---|---|---|---|---|
| shared tools with `expose_spec: true` that a sharee read | primary | 0 (no read path) | ≥ 1 in the first compete round-two run | mcphost calls log for `host.tool_spec_shared` | first live tournament |
| env values or secret refs in any `tool_spec_shared` response | guardrail | n/a | 0 | redaction test + live grep of saved responses | every release |
| `spec_not_exposed` refusals | secondary | n/a | reported | calls log | ongoing |

## Technical considerations

- Reuse the share membership check that `host.tool_call` on a `<owner_ns>.<tool>` path uses (or will use after PRD-mcphost-shared-tool-call-path); do not write a second membership resolver.
- Redaction lives next to the python kind's spec validation (`src/kinds/python.rs:335-341` lists the spec keys) so a new key added to the kind is redacted by allowlist, not by denylist: only `source`, `args_schema`, `requirements`, `timeout_s`, `network`, and for `http` kind `url`, `method`, `headers` minus any secret refs, are returned.
- The synthetic label on arena tenants is unaffected; this tool is available on every plan, Free included (Joe 2026-09-26); no plan check is added.
- Per the 2026-09-26 rhyme rule, the Live AC is a two-actor flow: owner shares with exposure, a second tenant reads.

## Migration / compatibility

Existing shares read as `expose_spec: false`; no behaviour changes until an owner opts in. Additive schema change with a default.

## Open questions

| question | owner | due |
|---|---|---|
| Should `expose_spec` be plan-gated (Pro only) like `network: "egress"`? DECIDED 2026-09-26 (Joe): free on every plan; no plan check in this PRD. `spec_reads` (req 9) is where a paid analytics feature could live later. | Joe | closed |

## Acceptance criteria

1. P0 — Given an owner publishes a python tool and shares it to a group with `expose_spec: true`, When a group member calls `host.tool_spec_shared`, Then the response has `kind: "python"` and a `spec` containing `source` and `args_schema`.
2. P0 — Given the same share, When the owner calls `host.tool_list`, Then the tool row shows `expose_spec: true`.
3. P0 — Given a tool whose spec has `env: {"API_KEY": ...}` and a secret reference in `source`'s config, When a sharee reads it, Then the response contains no `env` key and no secret reference string (asserted by the redaction test in requirement 3).
4. P0 — Given a tool shared without `expose_spec`, When a sharee calls `host.tool_spec_shared`, Then the response is `spec_not_exposed` naming `<owner_ns>.<name>`.
5. P0 — Given a tool not shared with the caller, When it calls `host.tool_spec_shared`, Then the response is the same not-found shape `host.tool_call` returns for an unshared tool.
6. P0 — Given a share with `expose_spec: true` that the owner then unshares, When the former sharee calls `host.tool_spec_shared`, Then not-found; and after re-sharing without the flag, `spec_not_exposed`.
7. P0 — Given `docs/agent-quickstart.md` and `www/llms.txt` after this PRD, When they are grepped, Then both contain `expose_spec` and `host.tool_spec_shared` with a worked example.
8. P0 — Given a 64 KiB spec, When 50 sharees read it concurrently, Then p95 < 100 ms and every response is identical.
9. P1 — Given three reads by sharees, When the owner lists tools, Then the row shows `spec_reads: 3` and an `exposed_at` timestamp.
10. P0 — Given prod mcphost after deploy, When tenant A publishes a python tool with one `env` entry and shares it to a group containing tenant B with `expose_spec: true`, and tenant B calls `host.tool_spec_shared`, Then B receives the `source` and no `env`, and A's `host.calls.list` shows B's read (Live; evidence: the two-tenant transcript saved under the mcphost measure run, plus A's calls listing).
