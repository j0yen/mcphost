# PRD — mcphost-gate-debt-24d1794: inherited gate debt at 24d1794

- Status: building
- Lane: redbaron 2026-09-15T04:17:07Z pid=3529262 boot=c6865fd1-71c2-48cf-818e-5e1f2246b3fe
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_priority: high
- publish: none
- test_prefix: gatedebt-24d1794
- Vision: visions/buildloop-operations.md
- Drafted: 2026-09-14

## TL;DR

At HEAD 24d1794, the gate on mcphost blocked on 1 inherited
finding(s) — landed by earlier merges, not by the PRD that was gate-pending
when this was drafted (PRD-build-gate-debt-auto-prd requirement 2:
attribution split these from that PRD's own diff). This PRD's acceptance
criteria are exactly those findings; when they pass, the parked PRD
unblocks automatically (its `Depends-on:` resolves once this archives).

## Problem statement

Five whys (one per inherited finding, from `git log -S` where a candidate
commit was found):

- reviewer-agent: introduced by `24d1794 mcphost: fix gate paper trail for mcphost-gate-debt-a7d8e1c` (git log -S match on the finding text).

## Goals

1. Every inherited finding listed below passes the gate at a HEAD that
   includes this PRD's fix.

## Non-goals

- Does not change gate/verdict semantics or re-derive attribution.

## Acceptance criteria

1. P0 — Given HEAD 24d1794198ab6a7cbf5b828bd2fee8fa53e0c6b0, When the gate runs, Then reviewer-agent passes: finalize rejected the subagent's output
