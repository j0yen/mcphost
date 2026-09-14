# PRD — mcphost-gate-debt-a7d8e1c: inherited gate debt at a7d8e1c

- Status: building
- Lane: redbaron 2026-09-14T09:44:15Z pid=749265 boot=c6865fd1-71c2-48cf-818e-5e1f2246b3fe
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_priority: high
- publish: none
- test_prefix: gatedebt-a7d8e1c
- Vision: visions/buildloop-operations.md
- Drafted: 2026-09-14

## TL;DR

At HEAD a7d8e1c, the gate on mcphost blocked on 1 inherited
finding(s) — landed by earlier merges, not by the PRD that was gate-pending
when this was drafted (PRD-build-gate-debt-auto-prd requirement 2:
attribution split these from that PRD's own diff). This PRD's acceptance
criteria are exactly those findings; when they pass, the parked PRD
unblocks automatically (its `Depends-on:` resolves once this archives).

## Problem statement

Five whys (one per inherited finding, from `git log -S` where a candidate
commit was found):

- reviewer-agent: introduced by `5511083 mcphost: add AC1 test for mcphost-gate-debt-6d51e76 (fix ac-number-collision)` (git log -S match on the finding text).

## Goals

1. Every inherited finding listed below passes the gate at a HEAD that
   includes this PRD's fix.

## Non-goals

- Does not change gate/verdict semantics or re-derive attribution.

## Acceptance criteria

1. P0 — Given HEAD a7d8e1c9b8338b8ca9b663f930d041542f3d6979, When the gate runs, Then reviewer-agent passes: finalize rejected the subagent's output
