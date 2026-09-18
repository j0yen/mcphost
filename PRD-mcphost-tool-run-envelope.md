# PRD: mcphost-tool-run-envelope — the dry-run surface reads like the call surface

- Status: queued
- Lane: redbaron 2026-09-15T22:36:09Z pid=3056040 boot=c6865fd1-71c2-48cf-818e-5e1f2246b3fe
- Blocked: gate-red at worktree HEAD e576fe26a485606721291c31fc8467adc6de0c0a (branch `autobuilder/mcphost-tool-run-envelope`, base a1fcdba) — `gate-then-land.sh` exit 7 (land-ungated); branch kept, main untouched, nothing landed. `extend-gate.sh --scope branch` verdict: pass=22 block=3 (reviewer-agent, rollback-plan, ci-checks); gate-attribution classified all three `new_blocks` (inherited_blocks=none). Independently re-verified rather than trusting that label: reviewer-agent is a pure cascade ("2 block(s) already recorded (no Sonnet spend on a red gate)"), not a third independent finding. rollback-plan's own message is `tag v0.53.3 already exists on a different commit; cannot redeploy-tag HEAD` — checked directly: `git rev-parse v0.53.3` = `34eb75b` (an ancestor of this branch's base), while `main` has since moved to `fe7afa7` ("sync Cargo.lock self-version to 0.54.1", landed by a sibling PRD this same tick); this branch's own worktree Cargo.toml is still unbumped at 0.53.3 (by design — shared-target branches commit implementation-only, deferring the bump to `integrate`), so redeploy-tag's model collides with the real `v0.53.3` tag object that already exists in the (shared) `.git`. ci-checks' message is `no workflow runs found for HEAD; push the commit and wait for CI` — this branch has never been pushed (also by design, pre-integrate). Both read as structural to gating an unbumped, unpushed shared-target branch commit via `--scope branch`, not caused by this PRD's 4-file diff (`src/kinds/python.rs`, `src/handler.rs`, `src/control.rs`, four `tests/runenvelope_ac*.rs` files) — gate-attribution's in-scope/inherited git-blame heuristic has no file to blame for a tag-collision or an unpushed-branch check, so it likely defaults such non-file-scoped receipts to in-scope rather than genuinely finding them caused by this diff. Not something this PRD's own scope (a tool_run result-shape patch) can or should fix forward. Per the exit-7 contract: did not retry, did not tag/archive, did not write a passing Receipts line. Worktree/branch left in place (`worktree-extend.sh cleanup` NOT run) for a future attempt once main settles past this tick's concurrent shared-target landings — likely resolves on its own once a sibling PRD's `integrate` bumps+tags+pushes this branch's own base forward.
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_priority: low
- build_version_bump: patch
- publish: j0yen/private
- test_prefix: runenvelope
- deferred_acs: [5]
- mock_unjustified_for: [5]
- mock_justifications:
  - "AC5 (P1 — CHANGELOG names the old shape and version boundary) has no branch-local test possible: this is a shared-target rust-extend PRD, so the version bump and CHANGELOG.md prepend happen at worktree-extend.sh integrate time (serial, locked), not in this branch's own worktree/commit — the branch commits implementation-only, per the Worktree isolation contract. The archive gate's own rust-extend Check #3 (CHANGELOG.md exists with a `## v<new-version>` section at top containing the PRD's TL;DR) verifies this at ship time instead; the TL;DR handed to `extend-handler.sh changelog-prepend` at land time names the old `{duration_ms, exit_code, result}` shape and the version it changed in, satisfying the AC's content requirement without a Rust test asserting it prematurely against a file this branch never writes."
- Vision: visions/mcp-host.md
- Loop: mcphost-buildloop: satisfaction
- Grounding: failure-derived — five-whys not required at this depth (single located mechanism); evidence and mechanism in visions/mcp-host.md, 2026-09-08 measure note (admin_agent diagnosis)
- PM: Joe Yen
- Drafted: 2026-09-12
- Engineering target: extend `~/wintermute/mcphost` — `host.tool_run`'s result shape, its descriptor text, the envelope tests
- iter_log: 2026-09-15T22:5xZ (redbaron) chained_steps=1. Implementation was already sitting on `autobuilder/mcphost-tool-run-envelope` (commit e576fe2) from the stale reclaimed lane claim — read and independently re-verified rather than trusted: `cargo check --tests` clean; all 4 new `tests/runenvelope_ac*.rs` (AC1-AC4) plus the modified `warmpool_ac06_ac07_tool_run` and the `surface_ac02`/`surface_ac05` descriptor/llms.txt-parity tests re-run fresh in this dispatch, all green (`suite_sandbox_02`/`suite_sandbox_03`/`suite_core_07`, real bwrap-sandboxed python calls, not CI-skipped). Deferred AC5 (frontmatter above) and reclaimed the stale `Lane:` via `lane-claim.sh claim` before touching state. Ran `gate-then-land.sh /home/jsy/wintermute/mcphost mcphost-tool-run-envelope patch <tldr>` in the foreground: exit 7 (land-ungated) — see `Blocked:` above for the full gate readout and independent analysis. chain-guard.sh not invoked (a failed step's own `blockers` already stops the chain per its own table; nothing was landed to re-check preconditions against). No `manifest-set.sh` write for `status: shipped` — never reached that step.

## TL;DR

`host.tool_run` returns `{duration_ms, exit_code, result: {…}}` with no `payload`
envelope, while every real call returns `result.payload` per the envelope contract.
One measured session paid for the difference: `panel_admin_agent_02` scored 0 because
the judge read the dry-run's shape as undocumented even though the tool worked
(vision, 2026-09-08 admin_agent diagnosis). Joe pulled the recorded question forward
on 2026-09-12: make the dry-run result follow the same envelope, so an agent's
mental model — and the judge's — carries across the two surfaces unchanged.

## Problem statement

The envelope contract (PRD-mcphost-result-envelope-contract) exists because "a
correct field at an unpredictable path" was the largest recoverable satisfaction
deficit in the 0.26.3 runs — 4 of 21 sessions docked. The contract was applied to
calls; `host.tool_run` kept its pre-contract shape. The cost is measured (one session
zeroed at 0.27.0) and the confusion is structural: four dry-run-adjacent tools
already needed a decision table to distinguish (stress audit, surface-fluidity), and
the one that executes real code returns a shape none of the others share. An agent
that tests before publishing — the exact behavior the product wants to encourage —
sees two shapes for one tool.

## Goals

- `host.tool_run`'s result carries `result.payload` (with declared-output promotion)
  exactly as `host.tool_call` does, plus its run-metadata fields.
- The change is visible in descriptors and docs, and the envelope test suite covers
  both surfaces from one fixture set.

## Non-goals

- No change to `tool_run`'s no-metering property (by design it records no `calls`
  row — that stays).
- No changes to the other dry-run tools (`tool_test`, `bridge_test`, `spec_test`
  render requests; they execute nothing and have no result envelope to align).

## User stories

1. **Agent iterating on a tool.** When I `tool_run` before publishing, I want the
   result at the same path a real call will use, so my extraction code is written
   once and is correct on the first real call.
2. **The judge.** When a session exercises the dry-run, I want the transcript shape
   to match the documented contract, so a working tool is never scored as
   undocumented output.

## Requirements

**P0**
1. `host.tool_run`'s result places tool output under `result.payload` with the same
   declared-output promotion the call path applies; `duration_ms`, `exit_code`, and
   any existing run metadata remain as sibling fields.
2. The envelope test fixtures run against both `tool_call` and `tool_run`, asserting
   identical payload placement and promotion for identical tool output.
3. Descriptor text and llms.txt state that `tool_run` returns the standard envelope
   plus run metadata; the dry-run decision table (surface-fluidity lineage) is
   updated in place.

**P1**
4. A compatibility note in the changelog names the old shape and the version boundary
   (agents with code against the old shape are all synthetic today — 09-09 audit:
   `host.tool_run` was not called once in 42 sessions — but the note costs one line).

## Success metrics

| metric | baseline | target | method | timeframe |
|---|---|---|---|---|
| Envelope parity across call/dry-run | divergent (1 session zeroed) | identical placement on shared fixtures | runenvelope ACs | on ship |
| Judge misreads of dry-run shape | 1 of 84 rows | 0 on next comparable run | ledger reasons | next run exercising tool_run |

## Technical considerations

Reuse the call path's promotion function at the `tool_run` result site — one shared
code path, not a copied one, so the two surfaces cannot drift again. Low priority and
patch-sized by design; it should ride a quiet lane.

## Migration / compatibility

Breaking for consumers of the old dry-run shape; the production audit found zero
callers in 42 recorded sessions and no external tenants exist. Version bump and
changelog note cover it.

## Open questions

None.

## Acceptance criteria

1. P0 — Given a tool whose output matches its declared outputs, When exercised via `tool_call` and via `tool_run` on the same fixture, Then both results carry the fields at `result.payload.<field>` identically, and `tool_run` additionally carries `duration_ms` and `exit_code`.
2. P0 — Given a tool with no declared outputs, When `tool_run` executes it, Then raw output lands under `result.payload` exactly as a call would place it.
3. P0 — Given the descriptors and llms.txt after build, When read, Then `tool_run` documents the standard envelope plus run metadata and the dry-run decision table reflects it.
4. P0 — Given `tool_run` executes, When the calls table is inspected, Then no row was recorded (the no-metering property is unchanged, asserted by test).
5. P1 — Given the changelog after build, When read, Then the shape change and version boundary are named.
