# PRD: mcphost-tool-versions — a bad republish is undone in one call

- Status: queued
- Lane: orch 2026-09-22T19:39:49.472784029+00:00 run=119
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_priority: normal
- publish: j0yen/private
- Vision: visions/mcp-host.md
- Loop: mcphost-buildloop: satisfaction — helpfulness on the "undo a bad publish" task
- Grounding: wwhtbt — visions/mcp-host.md addendum 2026-09-18, leaf 3: `kinds/python.rs:41` republish overwrites; no version table in `migrations/`; no rollback tool; shared tools (`sharing.rs`) have callers who cannot pin
- PM: Joe
- Drafted: 2026-09-18
- deferred_acs: [6]
- mock_justifications: AC6 -- prod-only proof ("Given prod after ship ... `host.tool_history` on prod shows both versions"). It can only be satisfied by a live run against the deployed https://mcphost.dev with homeward's own bearer key (MCPHOST_HOMEWARD_KEY/MCPHOST_HOMEWARD_TOOL); those are operator-held production credentials a build agent is not authorized to obtain, and prod does not serve host.tool_history/host.tool_rollback until this branch ships, so no run in this worktree can execute the prod leg. The live check is written and ready for the operator's post-ship run, gated on MCPHOST_LIVE=1: tests/mcphost_tool_versions_ac06_live_rollback_trailer.rs::republish_then_rollback_leaves_both_versions_in_history, which prints the host.tool_history output for the trailer. The same test runs the identical publish/republish/rollback/history sequence against a real local mcphost server on every cargo test -- that proves the branch's mechanism, not AC6, and is not counted as AC6's proof.
- Engineering target: mcphost `src/handler.rs` (`host.tool_publish`, new `host.tool_history`, `host.tool_rollback`), `src/db.rs`, `src/sharing.rs`, migration `tool_versions`

## TL;DR

Agents iterate: publish, call, fix, republish. Today the fix replaces the tool and
the previous source is gone. If the fix is worse, the agent has nothing to go back to
and a caller of a shared tool sees the change with no warning. Every publish becomes a
version; the last N are kept per plan; `host.tool_rollback` restores one; a caller may
pin a shared tool to a version.

## Problem statement

The target persona is an agent that ships tools in minutes (docs/agent-quickstart.md:
signup→first publish 30.7 s median). Evidence: `on_tool_changed (republish/…)` in
`python.rs:41` rebuilds the env and discards the old source; `grep -rniE
'tool_version|previous_version|version_history' src` finds nothing; `sharing.rs`
lets tenant B call tenant A's tool by name with no version. Consequence: one wrong
republish breaks a running workflow for its owner and for every sharer, with no undo
short of pasting old source from a transcript.

## Goals

- Every publish is immutable and numbered; the current version is a pointer.
- Rollback in one call; history in one call.
- Sharers can pin a version; unpinned sharers get a `version_changed` note in the
  result envelope on first call after a change.

## Non-goals

- Branching or named tags beyond an integer version.
- Retaining sandbox envs for every version (envs are rebuilt on rollback).

## User stories

- As the owner, `host.tool_rollback {name, version: 3}` makes version 3 current.
- As the owner, `host.tool_history {name}` lists versions with timestamps and a
  source hash.
- As a sharer, I call `{name, version: 3}` and keep working while the owner iterates.
- As the plan, `free` keeps 5 versions and `pro` keeps 20.

## Requirements

P0
1. Migration `tool_versions(tenant, name, version INTEGER, source, kind, config,
   requirements, created_unix, source_sha256)`; `tools` gains `current_version`.
2. `host.tool_publish` inserts a new version and advances `current_version`; the
   previous version stays; oldest versions beyond the plan's `versions_max` (`free` 5,
   `pro` 20, in `plans.rs`) are deleted oldest-first.
3. `host.tool_history {name}` returns versions newest-first with `version`,
   `created`, `source_sha256`, `current: bool`.
4. `host.tool_rollback {name, version}` sets `current_version` and triggers the kind's
   `on_tool_changed`; returns the new current version; refuses an unknown version.
5. `host.tool_call` and cross-tenant calls accept optional `version`; a pinned call
   runs that version; an unknown version is an argument error.

P1
6. A shared tool's unpinned caller receives `version_changed: {from, to}` once in the
   result envelope after the owner's publish or rollback (per caller, per change).
7. `host.tool_remove` deletes all versions; `host.tool_list` shows `current_version`
   and `versions`.

P2
8. `host.tool_diff {name, from, to}` returns a unified diff of source.

## Success metrics

| metric | baseline | target | method | timeframe |
|---|---|---|---|---|
| republishes with a recoverable previous version | 0 % | 100 % | `tool_versions` rows | at ship |
| synthetic panel task "undo a bad publish" satisfaction | no task | ≥ 0.9 | new consumer task in synthorg corpus | first run after ship |

## Technical considerations

- Sandbox env cache is keyed by requirements hash (`python.rs:15`), so rollback to a
  version with the same requirements reuses the env.
- Retention per plan lives with the other quotas in `plans.rs`/`tenant_state.rs`.
- Result envelope additions follow PRD-mcphost-result-envelope-contract (additive
  field).

## Migration / compatibility

Existing tools become version 1 on migration; `current_version = 1`.

## Open questions

| question | owner | due |
|---|---|---|
| versions_max per plan (assumed 5 / 20) | Joe | at build |

## Acceptance criteria

1. P0 — Given a tool published twice, When `host.tool_history` is called, Then two versions are listed, version 2 current, with distinct `source_sha256`.
2. P0 — Given version 2 current, When `host.tool_rollback {version: 1}` is called, Then the next `host.tool_call` runs version 1's source.
3. P0 — Given a `free` tenant with 5 versions, When a sixth is published, Then version 1 is deleted and versions 2–6 remain.
4. P0 — Given `host.tool_rollback {version: 9}` for a tool with 2 versions, When called, Then an argument error names the valid range.
5. P0 — Given a shared tool at version 3 and a caller pinning `version: 2`, When the owner publishes version 4, Then the pinned call still runs version 2.
6. P0 — Given prod after ship, When homeward's tool is republished and rolled back once during the build's live check, Then `host.tool_history` on prod shows both versions (proof: the live call output in the trailer).
7. P1 — Given an unpinned sharer, When the owner rolls back, Then the sharer's next result carries `version_changed` once and not on the following call.
8. P1 — Given `host.tool_remove`, When called, Then all versions are gone and `host.tool_history` returns not-found.
9. P2 — Given two versions, When `host.tool_diff` is called, Then a unified diff is returned.
- iter_log: 2026-09-22T08:12:41.939534456+00:00 decision 22 answered: infra runs were the old binary + systemd-run driver EACCES (reverted); re-admit
- iter_log: 2026-09-22T17:09:30.179178365+00:00 decision 32 answered: wt Cargo.lock lockfile noise (mcphost version 0.58.5→0.59.2) discarded; retry land
