# PRD — mcphost-gate-debt-c627803: inherited gate debt at c627803

- Status: building
- Lane: redbaron 2026-09-13T21:26:31Z pid=2697991 boot=c6865fd1-71c2-48cf-818e-5e1f2246b3fe
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_priority: high
- publish: none
- test_prefix: gatedebt-c627803
- Vision: visions/buildloop-operations.md
- Drafted: 2026-09-13

## TL;DR

At HEAD c627803, the gate on mcphost blocked on 1 inherited
finding(s) — landed by earlier merges, not by the PRD that was gate-pending
when this was drafted (PRD-build-gate-debt-auto-prd requirement 2:
attribution split these from that PRD's own diff). This PRD's acceptance
criteria are exactly those findings; when they pass, the parked PRD
unblocks automatically (its `Depends-on:` resolves once this archives).

## Problem statement

Five whys (one per inherited finding, from `git log -S` where a candidate
commit was found):

- extended-receipts: introducing commit not identified by `git log -S` (finding text too generic or the line predates this repo's history here).

## Goals

1. Every inherited finding listed below passes the gate at a HEAD that
   includes this PRD's fix.

## Non-goals

- Does not change gate/verdict semantics or re-derive attribution.

## Acceptance criteria

1. P0 — Given HEAD c627803598ee85ae3795ec3f7ee21a50c31adee0, When the gate runs, Then extended-receipts passes: one or more extended producers did not pass|skip (see output above)

