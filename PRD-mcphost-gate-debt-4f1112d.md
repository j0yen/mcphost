# PRD — mcphost-gate-debt-4f1112d: inherited gate debt at 4f1112d

- Status: queued
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_priority: high
- publish: none
- test_prefix: gatedebt-4f1112d
- Vision: visions/buildloop-operations.md
- Drafted: 2026-09-16

## TL;DR

At HEAD 4f1112d, the gate on mcphost blocked on 3 inherited
finding(s) — landed by earlier merges, not by the PRD that was gate-pending
when this was drafted (PRD-build-gate-debt-auto-prd requirement 2:
attribution split these from that PRD's own diff). This PRD's acceptance
criteria are exactly those findings; when they pass, the parked PRD
unblocks automatically (its `Depends-on:` resolves once this archives).

## Problem statement

Five whys (one per inherited finding, from `git log -S` where a candidate
commit was found):

- vti-plan: introducing commit not identified by `git log -S` (finding text too generic or the line predates this repo's history here).
- rollback-plan: introducing commit not identified by `git log -S` (finding text too generic or the line predates this repo's history here).
- extended-receipts: introduced by `cc5e7a8 agent: refresh intent card for mcphost-gate-debt-a1fcdba` (git log -S match on the finding text).

## Amendment 2026-09-16 (evidence from the 18:46Z post-land main gate at 9bfcf85)

Failure under this seed: yes — mcphost main is 7 commits ahead of origin and unpushed because the post-land main-scope gate blocks on `extended-receipts` (flake-audit) at every HEAD since a1fcdba landed.

Five whys, each with its observation:

1. Why is main unpushed? `gate-then-land.sh` pushes only after the post-land main gate passes; it exited 11 (`post-land-main-gate-block`) at 18:46:15Z with blockers `ci-checks` (no run exists for an unpushed HEAD) and `extended-receipts`.
2. Why does extended-receipts block? `target/autobuilder/receipts/flake-audit-receipt.json` on main: `verdict: block, deterministic: true, exit_codes: [101,101,101], head_sha: 9bfcf85`.
3. Why does `cargo test` exit 101 three times? Exactly one test fails every run: `gatedebt_a1fcdba_ac1_extended_producers_not_blocked::flake_audit_does_not_block` (receipt `2026-09-16-mcphost-gate-debt-4f1112d-flaky-infra.txt`: `214 passed; 1 failed`).
4. Why does that test fail? `tests/gatedebt_a1fcdba_ac1_extended_producers_not_blocked.rs` reads `target/autobuilder/receipts/flake-audit-receipt.json` from the PREVIOUS gate run and asserts its verdict is pass|skipped. The receipt it reads is the one its own failure produced. The loop is closed: one block receipt on disk makes the test fail forever, and the test failing writes the next block receipt.
5. Why did that test exist? PRD-mcphost-gate-debt-a1fcdba's agent wrote a test that asserts on a gate artifact instead of on repo behaviour (an agent-written fixture that restates the gate verdict). The original a1fcdba receipt was `deterministic: false` — a genuinely flaky run — and the test turned a one-off flake into a permanent main-scope block.

Consequence: no mcphost PRD can ship (branch gates pass on fresh worktrees, which have no stale receipt; the post-land main gate never does), and the dispatch of this PRD on 12:23Z filed the failure as "flaky-infra" and declined to change code.

Deepest actionable level is 4: no test in this repo may read `target/autobuilder/receipts/`. Delete the whole test file (both `flake_audit_does_not_block` and `cold_build_time_does_not_block` have the same shape); the gate already enforces those verdicts itself. Do not skip, ignore, or retry the test, and do not edit the receipt by hand — the receipt must turn pass because the suite passes.

Status of the original ACs at the time of amendment: AC1 (vti-plan) passes on main since PRD-mcphost-proof-lane-loop-config landed at 18:36Z (branch gate `vti-plan:1` clean); AC2 (rollback-plan) is scope-deferred by the reinstalled tag-aware producer; AC3 is the loop above.

## Goals

1. Every inherited finding listed below passes the gate at a HEAD that
   includes this PRD's fix.

## Non-goals

- Does not change gate/verdict semantics or re-derive attribution.

## Acceptance criteria

1. P0 — Given HEAD 4f1112df8af5183c47807ff2fa50be0453c9d774, When the gate runs, Then vti-plan passes: autobuilder vti-plan exited non-zero
2. P0 — Given HEAD 4f1112df8af5183c47807ff2fa50be0453c9d774, When the gate runs, Then rollback-plan passes: commits since v0.54.1 are not all revert-clean (see target/autobuilder/rollback.md); fix forward, or a human rewrites history and says so — this script never edits history to force a pass
3. P0 — Given HEAD 4f1112df8af5183c47807ff2fa50be0453c9d774, When the gate runs, Then extended-receipts passes: one or more extended producers did not pass|skip (see output above)
4. P0 — Given the repository tree after this PRD's change, When `grep -rl "target/autobuilder/receipts" tests/` runs, Then it prints nothing (no test reads a gate receipt).
5. P0 — Given the branch worktree, When `cargo test --quiet` runs three times in a row, Then every run exits 0 and no test named `flake_audit_does_not_block` or `cold_build_time_does_not_block` exists.
6. P0 — Given main after this PRD lands, When the post-land main-scope gate runs, Then `flake-audit-receipt.json` has `verdict: pass` with `exit_codes: [0,0,0]`, `extended-receipts` is not in the blockers, and `gate-then-land.sh` pushes main (origin/main equals local main).
7. P1 — Given the changelog for this PRD, When it is read, Then it quotes the main-scope gate line and the flake-audit receipt fields from AC6.
- iter_log: 2026-09-16T19:05Z amended by /dream (Joe: "amend the prd now", post-land main gate block at 9bfcf85) — added five-whys amendment naming the circular test file; appended AC4–AC7; AC1–AC3 unchanged.
