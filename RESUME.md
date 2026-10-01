# mtproof checkpoint (RESUME)

## Branch state
- feat/mtproof @ f48b31d, pushed to origin, matches origin/feat/mtproof exactly (clean worktree).
- PR #65 open against master: https://github.com/j0yen/synthorg/pull/65 -- mergeStateStatus=CLEAN, mergeable=MERGEABLE (verified after rebase onto origin/master 6aea8a7, which includes journey #61, promise #62, conformance #63, journey state.put fix #64).
- NOT yet merged (no "gh pr merge" run).

## Requirements done
1-7 (P0) + 8 (P1): all implemented in src/synthorg/mtproof.py (5 probes: kill_switch, signup_limiter, egress_allowlist, plugin_snippets, claim; wave rehearsal; readiness digest wave1/wave2 per Joe's finalized rule) + chain.py/chain_preconditions.py/chain_ledger.py/chain_report.py/cli.py wiring (--mtproof flag, weekly-Sunday default in deploy/nightly-chain.sh reference file -- NOT yet installed over ~/.local/bin/nightly-chain.sh, operator must do that manually).
call_tool/run_snippet/claim_start on JourneyHost intentionally raise NoDocumentedPathError (live mechanism undocumented) -- scoped intentionally per PRD's "no invented endpoints" rule.

## Requirements NOT done
- Requirement/AC 11: deferred_acs per PRD frontmatter (not in scope this pass).
- Live AC 12 (prod probe run via "mtproof run --probe all" under the nightly-chain systemd unit env): NOT started. Blocked on: (a) PR #65 merge, (b) nightly chain unit confirmed inactive, (c) prod checkout ~/repos/synthorg pulled to/past the squash + uv sync. Do NOT touch ~/repos/synthorg until merge lands and chain is idle.

## Tests
- tests/mtproof_ac01..ac10_*.py (21 hermetic tests) + full chain-related group (tests/mtproof_*, journey_*, chainrun_*, linkpre_*, fixturepreflight_*, chainledger_*) = 127/127 passed, verified fresh after the post-rebase conflict resolution (ran ~04:40, 43s).
- ruff clean on all touched files (mtproof.py, chain.py, chain_preconditions.py, chain_ledger.py, chain_report.py, cli.py, tests/mtproof_*.py). mypy clean on mtproof.py.
- Full repo-wide suite ("pytest -q -m 'not flaky'"): launched twice. First run (pre-second-rebase) was killed deliberately before finishing (had to rebase onto a newer origin/master that landed mid-run -- stale worktree). Second run launched ~04:43 (PID 1909856, log at /home/jsy/tmp/mtproof-fullsuite2.log) -- IN FLIGHT, not yet confirmed complete. Early signal from the first (killed) run: only a handful of F's, all in alphabetically-early files unrelated to mtproof/chain (never identified by exact name -- this is the one open thread).

## Next three steps
1. Check /home/jsy/tmp/mtproof-fullsuite2.log for completion; if failures exist, name them and confirm pre-existing vs mtproof-caused via a clean "git worktree add --detach <path> origin/master" comparison (method already proven earlier this session) before trusting any red.
2. If suite is clean (or reds confirmed pre-existing/unrelated): "gh pr merge 65 --squash" (repo j0yen/synthorg, NO --admin).
3. After merge: wait for nightly-chain.service to go idle AND ~/repos/synthorg to be pulled to/past the squash sha + "uv sync", then run Live AC 12 via systemd-run against prod exactly as specified in the PRD ("mtproof run --probe all" only, never the wave). Kill-switch probe must leave the switch as found.
