# PRD: build-land-conflict-resolver — rebase before the gate, regenerate or union at land, and resolve source conflicts in the same dispatch

- Status: queued
- build_target: shell
- build_into: /home/jsy/wintermute/build-skill
- build_priority: high
- publish: none
- test_prefix: landres
- Vision: visions/buildloop-operations.md
- PM: Joe Yen
- Drafted: 2026-09-17
- Grounding: failure-derived — five whys in visions/buildloop-operations.md addendum 2026-09-17 17:10Z (gate-debt-4f1112d land exit 10 on agent/intent-card.json; agent-consent land-conflict 01:45Z on 11 files, 4 of them generated or append-only)
- Engineering target: ~/wintermute/build-skill — scripts/gate-then-land.sh (order; conflict path :434-440), scripts/worktree-extend.sh integrate (:682 rebase, :720-754 exits 4/5), new scripts/land-resolve.sh, new per-repo `state/land-policy/<repo>.json` (generated + append-only lists), mcphost `.gitattributes`
- Successor context: memory note build-shared-target-stale-base-recovery ("rebase + union-resolve") — advice to agents, never a mechanism; PRD-build-same-target-cap (cap 2-6 parallel branches per target since 2026-09-16) is what made collisions routine.

## TL;DR

A branch that passes its gate and then fails to rebase onto main is parked until a later tick re-admits it, and that retry reruns the same failing rebase: one to three hours lost per conflict, and gate-debt-4f1112d lost a shipped state to a single generated file (`agent/intent-card.json`). The gate ran on a head already behind main, so the conflict surfaced last. This PRD moves the rebase before the branch gate (so the verdict covers the tree that will land), teaches land to regenerate files a generator owns and union-merge append-only files instead of merging them textually, and, for true source conflicts, runs one bounded resolve inside the same dispatch (rebase, coder on sonnet with the crate's tests as the check, delta gate, land) before giving up. A per-repo policy file lists which files are generated and which are append-only; a ledger counts conflicts by file so the policy grows from evidence.

## Problem statement

**Who:** the build loop on RedBaron landing `rust-extend` (and worktree-isolated shell/python) branches into a target that other branches also land into; the operator waiting for a green ship.

**What they cannot do today:** land a gated branch when main moved under it. Observed 2026-09-17: gate-debt-4f1112d branch gate `pass` 12:25 PM EDT, then `gate-then-land.sh … exit 10 (worktree-extend.sh integrate unexpected exit 4, real rebase conflict in agent/intent-card.json, not retried)`; 01:45Z agent-consent `land-conflict` on src/db.rs, src/handler.rs, gen-test-suites.sh output, agent/intent-card.json, extended-gates.toml, 7 tests, www/llms.txt (landed by hand hours later). gate-then-land.sh:434-440 aborts and `die 8 … not retried`; worktree-extend.sh:720 `retry next tick`. `.gitattributes` in mcphost names only `Cargo.lock merge=ours`.

**Why they cannot:** the land model treats every file as source and every conflict as fatal; the rebase happens after the gate, inside integrate; the only retry is the next admission, which reruns the identical rebase.

**Consequence:** with 2-6 parallel branches per target, generated-artifact collisions are the norm, not the exception. Each costs a full gate wall (~25 min) plus one to three hours of queue latency, and the same slug can lose several ticks in a row. Two incidents in two days on the loop's most important repo.

**Failure under this seed:** yes — the sidecar and journal lines above; `git -C ~/wintermute/mcphost show HEAD:.gitattributes` → one line; `grep -n 'die 8' scripts/gate-then-land.sh` → :440.

## Goals

- The branch gate runs on the tree that will actually land.
- Generated and append-only files never produce a rebase conflict.
- A true source conflict gets one bounded resolve attempt in the same dispatch.
- The operator can see which files conflict and how often.

## Non-Goals

- Changing the same-target cap or serializing branches (parallelism stays).
- Auto-resolving conflicts in hand-written source without tests as the check.
- Rewriting the PR path or the pinned main verdict (other PRDs).
- Retroactively landing today's two conflicted branches by hand (the resolver's first live run does it).

## User stories

1. **Loop (pre-gate rebase):** as gate-then-land, before launching the branch gate I rebase the worktree onto current main; if that rebase conflicts I go straight to resolve, and the gate runs once on the rebased head.
2. **Loop (generated file):** as land, when the conflict is in a file the repo policy lists as generated, I take main's version, rerun the generator, commit the result, and continue.
3. **Loop (append-only file):** as land, when the conflict is in a file listed append-only, I union-merge and continue.
4. **Loop (source conflict):** as land, when a source or test file conflicts, I dispatch one sonnet coder in the worktree with the conflict markers, the PRD's ACs, and the crate's tests as the check; if tests pass I delta-gate and land; if not I record `land-conflict-unresolved:<files>` and stop, as today.
5. **Operator:** as Joe, `state/land-conflicts.jsonl` tells me which files conflict most, so I add them to the policy or fix the generator.
6. **Builder of a new target repo:** as the next PRD adding a repo, I write its `land-policy` file once.

## Requirements

**P0**
- R1. Pre-gate rebase: gate-then-land.sh rebases the branch worktree onto the target's current default tip before the branch gate; the gate's `base` is that tip. If main advances again between gate and land, integrate re-rebases; a conflict-free re-rebase whose tree equals the gated tree lands without re-gating, otherwise a delta gate runs (existing gate-delta.sh).
- R2. Policy file `state/land-policy/<repo-basename>.json`: `{"generated": [{"path": "...", "regen": "<command>"}], "append_only": ["..."]}`. mcphost's initial policy: generated = `agent/intent-card.json` (regen: intent-card-refresh for the landing slug), `tests/suite_*.rs` (regen: `scripts/gen-test-suites.sh`); append_only = `CHANGELOG.md`, `www/llms.txt`, `.buildloop/extended-gates.toml`. Missing policy → today's behavior (no regeneration, no union).
- R3. `scripts/land-resolve.sh <repo> <slug>`: on a conflicted rebase, for each conflicted path: generated → checkout theirs (main), run `regen`, `git add`; append_only → union merge (git's `merge=union` driver via a temporary attributes file, or the equivalent three-way union), `git add`; anything else → collected as source conflicts. Continues the rebase when no source conflicts remain.
- R4. Source conflicts: one bounded attempt — a `claude -p` coder on sonnet in the worktree with the conflict list, PRD ACs, and instruction to resolve and make `cargo test --workspace` (or the target's test command) pass; time-boxed by `LAND_RESOLVE_MAX_S` (default 900); on success the rebase continues and a delta gate runs before land; on failure or timeout the branch is restored to its pre-resolve state, sidecar `last_error=land-conflict-unresolved:<files>`, journal line, stop (today's behavior).
- R5. Ledger `state/land-conflicts.jsonl`: one record per conflict event: ts, repo, slug, files with their class (generated | append_only | source), resolution (regen | union | coder | unresolved), wall seconds. `scripts/land-conflicts-report.sh` prints files by frequency.
- R6. mcphost `.gitattributes` gains `merge=union` for the append-only paths in R2 (so an operator's manual rebase behaves the same way); committed through the normal mcphost landing path (a `loop/` PR), cited in receipts.
- R7. Selftest `scripts/land-resolve-selftest.sh` (fixture repo, stub coder): (a) two branches both regenerate a generated file → second lands via regen, no conflict record of class source; (b) two branches append different lines to an append-only file → union, both lines present; (c) a source conflict with a stub coder that resolves → delta gate runs, lands; (d) stub coder that fails → branch restored byte-identical, `land-conflict-unresolved` recorded, no partial land; (e) pre-gate rebase moves the gate base to main's tip; (f) missing policy file → old behavior; (g) ledger has one record per event with the right classes. Ends `PASS` with 0 FAIL.

**P1**
- R8. `worktree-extend.sh integrate` exit 4/5 paths call land-resolve.sh before dying, so python/shell worktree lands get the same behavior.
- R9. The daily digest / day ledger (if present) reports conflicts resolved by class.

Non-functional: regen + union resolve under 60 s; the whole resolve path (including the coder) fits inside the tick's RuntimeMax with the gate; selftest under 120 s on RedBaron.

## Success metrics

| metric | baseline | target | method | timeframe |
|---|---|---|---|---|
| primary: gated branches parked on a land conflict | 2 in 2 days (agent-consent, gate-debt) | 0 parked for generated/append-only causes; source conflicts resolved same-dispatch ≥ 80 % | `state/land-conflicts.jsonl` | 14 days after ship |
| primary: hours from branch-gate pass to landed for conflicted branches | 1–3 h (next admission) plus manual | < 30 min | journal timestamps | 14 days |
| secondary: gates run per landed branch | ≥2 when conflicted | 1 (+1 delta at most) | journal `gate` lines per slug | 14 days |
| guardrail: no partial or wrong land | none today | none (selftest d) | selftest + ledger `unresolved` records show branch restored | at ship |

## Technical considerations

- Reuse gate-delta.sh for the post-resolve gate; the pre-gate rebase means most lands need no delta at all.
- Regeneration must run with the landing slug's PRD (intent-card-refresh takes a slug); the policy `regen` command may use `{slug}`.
- Union merge semantics: fine for line-append files; wrong for TOML tables with the same key — extended-gates.toml is a list of paths in practice; the ledger will show if that assumption fails.
- The coder step uses the same identity and lock discipline as gate-then-land; one attempt, no loop.

## Migration / compatibility

- Repos without a policy file behave exactly as today (R2, selftest f).
- The first live run resolves the two currently conflicted mcphost branches when their slugs are next admitted.

## Open questions

| question | owner | due |
|---|---|---|
| Should the coder step be allowed for test files only, never `src/`, until the ledger shows it is safe? | Joe | after 5 source resolutions |
| Policy file in the target repo (`.buildloop/land-policy.json`) instead of build-skill state, so it travels with the repo? | Joe | at ship |

## Acceptance criteria

1. P0 — Given a fixture repo where main advanced after the branch was cut, When gate-then-land runs, Then the branch worktree is rebased onto main's tip before the branch gate and the gate's journal line shows `base=<main tip>`.
2. P0 — Given a policy listing `agent/intent-card.json` as generated with a regen command, and two branches that each regenerated it, When the second lands, Then no rebase conflict is recorded as class source, the regen command runs once, and the landed file equals the regen output.
3. P0 — Given a policy listing `CHANGELOG.md` as append-only and two branches that appended different lines, When the second lands, Then the landed file contains both lines and the ledger records class `append_only`, resolution `union`.
4. P0 — Given a source conflict and a stub coder that resolves it so tests pass, When land runs, Then the rebase continues, a delta gate runs, the branch lands, and the ledger records class `source`, resolution `coder`, with wall seconds.
5. P0 — Given a source conflict and a stub coder that fails (or exceeds `LAND_RESOLVE_MAX_S`), When land runs, Then the branch is byte-identical to its pre-resolve state, sidecar `last_error=land-conflict-unresolved:<files>` is written, the ledger records `unresolved`, and main is untouched.
6. P0 — Given no policy file for the repo, When a rebase conflicts, Then behavior matches today (abort, `land-conflict:<files>`, not retried) and no regen or union is attempted.
7. P0 — Given `scripts/land-conflicts-report.sh` and a ledger with several records, When it runs, Then it prints files ordered by conflict count with class and last resolution.
8. P0 — Given `scripts/land-resolve-selftest.sh` with fixtures for AC1–AC7, When it runs on RedBaron, Then it ends `PASS` with 0 FAIL.
9. P0 — Given this PRD has landed and mcphost's policy file exists, When the next mcphost branch on RedBaron hits a rebase conflict in a generated or append-only file, Then the journal shows `land-resolve … regen|union` for that slug and the branch lands in the same dispatch with no `not retried` line. (Live; evidence: `journal:land-resolve .* (regen|union)`; never deferrable — Joe 2026-09-17 no-defer live ACs.)
10. P1 — Given a python or shell worktree land (worktree-extend.sh integrate) that conflicts, When it runs, Then land-resolve.sh is invoked before exit 4/5 and the same classes apply.
11. P1 — Given mcphost's `.gitattributes`, When read after this PRD's mcphost PR merges, Then the append-only paths carry `merge=union` and the PR is cited in receipts.
