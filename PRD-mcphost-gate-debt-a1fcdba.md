# PRD — mcphost-gate-debt-a1fcdba: inherited gate debt at a1fcdba

- Status: building
- Lane: redbaron 2026-09-15T15:04:47Z pid=1500285 boot=c6865fd1-71c2-48cf-818e-5e1f2246b3fe
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_priority: high
- publish: none
- test_prefix: gatedebt-a1fcdba
- Vision: visions/buildloop-operations.md
- Drafted: 2026-09-15

## TL;DR

At HEAD a1fcdba, the gate on mcphost blocked on 1 inherited
finding(s) — landed by earlier merges, not by the PRD that was gate-pending
when this was drafted (PRD-build-gate-debt-auto-prd requirement 2:
attribution split these from that PRD's own diff). This PRD's acceptance
criteria are exactly those findings; when they pass, the parked PRD
unblocks automatically (its `Depends-on:` resolves once this archives).

## Problem statement

Five whys (one per inherited finding, from `git log -S` where a candidate
commit was found):

- extended-receipts: introduced by `c3278b7 mcphost: fix gate paper trail for mcphost-agent-directory + repoint AC test pointers` (git log -S match on the finding text).

## Goals

1. Every inherited finding listed below passes the gate at a HEAD that
   includes this PRD's fix.

## Non-goals

- Does not change gate/verdict semantics or re-derive attribution.

## Acceptance criteria

1. P0 — Given HEAD a1fcdbad66bb40d3afe8f46ab118bf8d8aba0f3d, When the gate runs, Then extended-receipts passes: one or more extended producers did not pass|skip (see output above)
