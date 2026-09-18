# PRD: mcphost-stdlib-pseudo-modules — tool_publish must not demand PyPI packages for `__future__`

- Status: queued
- iter_log: 2026-09-17T22:47:21Z reclaim: committed work found sha=f724fb6 (dead claim lane=redbaron pid=1786123 cause=dead-pid)
- build_priority: high
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- publish: none
- Vision: visions/mcphost-fleet-infrastructure.md
- Grounding: failure-derived — 2026-09-12 fleet-bridge-live AC6 live publish: `host.tool_publish` rejected the fleet adapter's `agent.py` with "cannot infer a PyPI requirement for import '__future__'"; receipts /home/jsy/brain/journal/build/receipts/2026-09-12-fleet-bridge-live-ac6-publish*.txt
- PM: Joe
- Drafted: 2026-09-12
- Engineering target: infer-data/python-stdlib.json + the requirement-inference path that consumes it

## TL;DR

The Python-kind requirement inference treats every import not in `infer-data/python-stdlib.json` as a PyPI dependency. The allowlist has no dunder/pseudo-module entries, so `from __future__ import annotations` — present in most modern Python source — makes publish fail with no workaround (an empty `requirements` list is correct for such a tool, and a non-empty one would be a lie). This blocks PRD-fleet-bridge-live's AC6 and will bite any tenant publishing idiomatic Python.

## Problem statement

`__future__` is a compiler-directive pseudo-module: always importable, never installable from PyPI. Similar always-present names the inference can encounter: `__main__`, and any future dunder the interpreter grows. Rejecting them turns valid source into a publish error that the calling agent cannot repair.

## Goals

- Idiomatic Python with pseudo-module imports publishes with `requirements: []`.

## Non-Goals

- Re-generating the full stdlib list from a different source (separate concern).
- Changing inference for genuinely unknown imports (still an error naming the import).

## Acceptance criteria

1. P0 — Given a python-kind spec whose source contains `from __future__ import annotations` and only stdlib imports, When it is published, Then publish succeeds with an empty inferred requirements list.
2. P0 — Given source importing `__main__`, When published, Then no PyPI requirement is inferred for it.
3. P0 — Given source importing a real unknown third-party name, When published with no requirements, Then the error still fires and names that import (regression guard).
4. P1 — Given the shipped stdlib JSON, When the test suite runs, Then a test asserts the pseudo-module entries are present, so a future list regeneration cannot silently drop them.

## Evidence

- Blocker text and receipts recorded in PRD-fleet-bridge-live.md (Blocked: 2026-09-12T23:38Z), root-cause detail in mcphost-fleet-bridge deploy/ROOT-STEPS.md.
- Unblocks: PRD-fleet-bridge-live AC6 (publish-fleet-tool.sh live against prod, after the next prod deploy carries the fix).
