# PRD — mcphost-proof-lane-loop-config: every tracked file routes to a proof lane, starting with the loop's own config

- Status: queued
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_priority: high
- build_version_bump: patch
- test_prefix: lanecov
- publish: j0yen/private
- Vision: visions/mcp-host.md
- Grounding: failure-derived — five-whys in visions/buildloop-operations.md (2026-09-16 addendum 9); journal 2026-09-16: `mcphost-mcphost-stdlib-pseudo-modules gate block … blocking=vti-plan — autobuilder vti-plan exited non-zero … phases=…vti-plan:0!,rollback-plan:defer,ci-checks:2… inherited=2 in-scope=0` at 01:57:41Z; worktree `receipts/vti-plan.json` routes `.buildloop/ci-equivalent.toml` at confidence 0.0
- Loop: mcphost-buildloop: gate_pass_rate
- PM: Joe
- Drafted: 2026-09-16
- Engineering target: extend ~/wintermute/mcphost — agent/proof-lanes.toml (nine `[[lane]]` entries, schema id/description/globs/required_commands), tests/ (new `lanecov_ac*` integration test), .buildloop/ci-equivalent.toml (unchanged content, now routed)

## TL;DR

`agent/proof-lanes.toml` gains a `loop-config` lane whose globs cover `.buildloop/**`, with `cargo test --workspace` as its required command. A new test walks every path in `git ls-files` and fails when any path matches no lane, so the next unrouted file is refused by CI at PR time instead of turning every branch gate red after it lands. Outcome: vti-plan routes the current main-vs-tag delta at confidence 1.0, and mcphost branch gates can pass again.

## Problem statement

Joe (operator) is holding a burst box down until an mcphost branch gate passes. Since 01:05Z none can: `autobuilder vti-plan` exits non-zero on every branch because `.buildloop/ci-equivalent.toml` — added to main by the loop's own push-gate PRD at 00:01Z (commit 4f1112d, the only commit since tag v0.54.1) — matches none of the nine lanes in `agent/proof-lanes.toml`, and vti-plan's `min_confidence` is 0.7. The block is inherited by every branch (gate attribution `inherited=2 in-scope=0`), the reviewer is skipped on the red, and gate-then-land exits 7. Three branches have cycled on it (stdlib-pseudo-modules 01:13Z and 01:57Z, test-suite-consolidation 02:04Z, tenant-self-offboard). The lane file has no test that says "every tracked file routes somewhere", so the gap was found by the gate, after the file was on main, by a different PRD's dispatch.

Failure under this seed: yes — vti-plan red on every branch since 01:05Z, cause an unrouted file on main (journal 2026-09-16).

## Goals

- `.buildloop/**` routes to a lane at confidence 1.0.
- CI fails when any tracked path routes to no lane.
- No change to what any existing lane requires.

## Non-goals

- Changing vti-plan's `min_confidence` or its routing rules (autobuilder is not this repo).
- Editing `.buildloop/ci-equivalent.toml` content.
- The build-skill cross-repo rule (PRD-build-cross-repo-commit-gate).

## User stories

- As the operator (Joe), when the loop adds a config file to mcphost, I want the branch gates to keep passing, so that I am not diagnosing a repo-wide red from a one-line omission.
- As a branch agent, when my diff touches nothing under `.buildloop/`, I want vti-plan to ignore main's own config commit, so that my gate measures my change.
- As the mcphost CI, when a PR adds a file no lane covers, I want a failing test naming the path, so that the lane is added in the same PR.

## Requirements

**P0**
1. `agent/proof-lanes.toml`: a `[[lane]]` with `id = "loop-config"`, a one-line description naming PRD-build-main-push-gate's CI-equivalent contract, `globs = [".buildloop/**"]`, `required_commands = ["cargo test --workspace"]`. Existing lanes unchanged byte-for-byte.
2. Coverage test `tests/lanecov_ac01_every_tracked_path_routes.rs`: reads `agent/proof-lanes.toml`, runs `git ls-files`, and asserts every path matches at least one lane glob (same glob semantics vti-plan uses: `globset` with `**`); on failure prints each unrouted path. Explicit allowlist in the test for paths that are intentionally unrouted, initially empty.
3. `cargo test --workspace` green at the landed head; `autobuilder vti-plan --base v0.54.1` on the landed head exits 0 with every route at confidence ≥ 0.7.

**P1**
4. The coverage test also asserts every lane's `required_commands` is non-empty and every glob compiles, so a malformed lane fails CI rather than vti-plan.

**P2**
5. `docs/agent-quickstart.md` (or the existing lane doc) gains one paragraph: how to add a lane when adding a new top-level path.

Non-functional: the test runs in under 2 s on the current tree (about 600 tracked files).

## Success metrics

| metric | baseline (2026-09-16 02:00Z) | target | method | when |
|---|---|---|---|---|
| mcphost branch gates blocked on vti-plan | 3 of 3 since 01:05Z | 0 | journal grep `blocking=vti-plan` | 24 h after land |
| Tracked paths with no lane | 1 (`.buildloop/ci-equivalent.toml`) | 0 | lanecov test | at land |
| Guardrail: existing lane routes | 9 lanes, unchanged | unchanged | diff of proof-lanes.toml minus the new block | at land |

## Technical considerations

- vti-plan reads `agent/proof-lanes.toml` from the worktree (receipt carries `proof_lanes_path` and its sha256), so a branch carrying the new lane routes its own delta correctly even before main has it.
- Glob matching must mirror vti-plan's; if the test's matcher and autobuilder's disagree, the test is wrong — prefer the same crate.
- The rollback base stays v0.54.1; this PRD's tag is the next patch.

## Migration / compatibility

Additive. Branches rebased onto the landed head route cleanly; branches not yet rebased still inherit the block until their next gate-then-land rebase (existing loop behaviour).

## Open questions

| question | owner | due |
|---|---|---|
| Should `loop-config` require the push-gate's own delta check (a script) in addition to `cargo test --workspace`? | Joe | after land |

## Acceptance criteria

1. P0 — Given the landed `agent/proof-lanes.toml`, When `autobuilder vti-plan --project . --base v0.54.1` runs on the landed head, Then exit 0 and the receipt's route for `.buildloop/ci-equivalent.toml` has confidence ≥ 0.7 with lane `loop-config`.
2. P0 — Given the current tree, When `cargo test --test lanecov_ac01_every_tracked_path_routes` runs, Then it passes.
3. P0 — Given a temporary tracked file `zz-unrouted/x.txt` in a scratch clone, When the coverage test runs, Then it fails and its output names `zz-unrouted/x.txt`.
4. P0 — Given the nine pre-existing lanes, When `proof-lanes.toml` is diffed against v0.54.1, Then the only change is the added `loop-config` block.
5. P0 — Given a branch worktree rebased onto the landed head with an empty diff, When `extend-gate.sh <wt> --scope branch` runs, Then the journal phases field shows `vti-plan:<n>` without `!`.
6. P1 — Given a lane with an empty `required_commands`, When the coverage test runs, Then it fails naming the lane id.
7. P2 — Given the docs after land, When grepped for `loop-config`, Then the how-to-add-a-lane paragraph is present.
