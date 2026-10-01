# RESUME — PRD-mcphost-tool-naming-convention-and-aliases

Branch: `feat/tool-naming-aliases`, pushed through commit `1be1b46`
("wip: tool-naming checkpoint", on top of `ccacad3`, the real squashed
feature commit). PR #91: https://github.com/j0yen/mcphost/pull/91
(open, not yet green — see below).

## Requirements done (1-5 P0, 6 P1) — all implemented in `ccacad3`

1. One rule (`host.<family>.<verb>`) + CI lint `scripts/tool-naming-lint.sh` — done.
2. 20 old names kept as forever-unbroken aliases, `src/tool_aliases.rs` — done.
3. `tools/list` advertises both names, alias carries `_meta["x-deprecated"]` — done.
4. `docs/tools.md` + `www/llms.txt` generated from live registry — done.
5. Alias-vs-canonical call metrics in `/healthz` (`tool_aliases`) + `docs/metrics.md` — done.
6. `did_you_mean` on near-miss unknown names + `naming_rule_url` on first `host.whoami` — done.
7. AC9 — deferred (P1, out of scope this pass), noted in PR body.
8. AC10 (live-prod evidence) — NOT YET DONE, blocked on merge + deploy.

## Tests written (all 13 sub-tests, confirmed green in isolation pre-rebase)

`tests/toolname_ac01..ac08_*.rs` (8 files) — all green as of the last
isolated run, AND as of the one full `cargo test --workspace
--no-fail-fast` run on the builder (`/cache/scratch/full-test-run-3.log`
on build@46.225.119.213): **0 failed across the entire workspace.**
That full-suite run happened BEFORE the merge below, so it has not been
rerun against the merged tree yet.

## Known state / what's unfinished RIGHT NOW

Main advanced past this branch's base (PR #88 "first-hour support
surface" landed, bumping to 0.65.0 independently — version numbers
collided and need reconciling). Did a `git merge origin/main` to
resolve. Conflicts and their resolution:

- `src/main.rs` — resolved, kept BOTH `ToolsDoc` (mine) and `GenDocs`
  (theirs) subcommands + match arms.
- `tests/suite_core_07.rs` / `suite_core_08.rs` — resolved by rerunning
  `./scripts/gen-test-suites.sh` (generated file, don't hand-merge).
- `docs/metrics.md` — resolved, kept both new sections (`tool_alias_usage`
  mine, `help_url_served{code}` theirs).
- `plugin/skills/mcphost/SKILL.md` — resolved, combined their
  tenant_key/no-reconnect behavioral fix with my canonical tool names.
- `README.md` — resolved (3 conflict blocks): combined their tenant_key
  step-3 rewrite with my canonical names; kept my "Leaving"/"Contributing"
  sections, spliced in their new "## Getting help" section verbatim,
  rewrote the now-stale "Contributing: naming a new host.* tool" section
  to reflect that the rename actually happened (was self-contradictory
  after mechanical renaming — fixed).
- `docs/agent-quickstart.md` — resolved, same pattern as README step 3/4.
- `www/llms-full.txt` — resolved by regenerating via
  `./scripts/gen-llms-full.sh` (generated file, don't hand-merge) —
  confirmed `--check` passes.

**THIS WAS ALL DONE AND COMMITTED in `1be1b46` — `git diff
--diff-filter=U` should be empty.** Verify that first on resume.

**IMPORTANT — scratchpad collision hazard discovered during this
checkpoint**: writing `RESUME.md` via the local scratchpad path and
`scp`-ing it got silently clobbered mid-flight by an UNRELATED session's
content (a `synthorg`/`mtproof` PRD checkpoint) that happened to land at
the exact same scratchpad file path between the Write and the scp. The
first push of `RESUME.md` (commit `9900df0`) therefore carries the WRONG
content. This file (committed in a follow-up commit) is the real one —
if `9900df0`'s `RESUME.md` is still wrong on resume, fix it with one more
commit; do not trust that filename's content on disk without checking
`git log -p -- RESUME.md` first.

**Cargo.lock / version**: this branch's `Cargo.toml` + `Cargo.lock` say
`0.65.0`. Main's PR #88 ALSO bumped to some version independently (check
`git show origin/main:Cargo.toml | grep version` — if it's also
`0.65.0` or higher, this branch's version needs bumping again to stay
ahead, e.g. `0.65.1` or `0.66.0`, plus `plugin/.claude-plugin/plugin.json`
kept in sync, plus `Cargo.lock` regenerated — run `cargo check` ON THE
BUILDER (46.225.119.213), never on orch).

## Alias table (20 entries, unchanged, source of truth = `src/tool_aliases.rs::ALIASES`)

host.key_rotate→host.key.rotate, host.self_offboard→host.self.offboard,
host.tool_publish→host.tool.publish, host.tool_list→host.tool.list,
host.tool_remove→host.tool.remove, host.tool_logs→host.tool.logs,
host.tool_test→host.tool.test, host.bridge_test→host.bridge.test,
host.tool_run→host.tool.run, host.tool_call→host.tool.call,
host.tool_history→host.tool.history, host.tool_rollback→host.tool.rollback,
host.tool_diff→host.tool.diff, host.tool_share→host.tool.share,
host.tool_spec_shared→host.tool.spec_shared, host.tool_unshare→host.tool.unshare,
host.secret_set→host.secret.set, host.secret_list→host.secret.list,
host.registry_publish→host.registry.publish, host.spec_test→host.spec.test.

## Next three steps

1. rsync `~/tmp/wt-naming` to the builder (`/cache/scratch/mcphost-naming`),
   run `cargo check --workspace` then the FULL `cargo test --workspace
   --no-fail-fast` again post-merge (the merge touched src/main.rs,
   README-derived generated files, test suite files — must reverify
   green, classify any new reds as caused-by-merge vs pre-existing).
2. Reconcile the version bump collision with main's PR #88 (see Cargo.lock
   note above), then `git add -A && git commit` (a real commit, not
   another wip) with message describing the rebase, push.
3. Check `gh pr checks 91` / `gh api repos/j0yen/mcphost/actions/runs
   --jq '.workflow_runs[] | select(.head_branch=="feat/tool-naming-aliases")'`
   — CI had NOT triggered at all before this checkpoint (0 runs found,
   `mergeable: CONFLICTING` on the PR, which may be WHY — recheck once
   pushed past the merge) — once green, squash-merge (NOT --admin), wait
   for the next hourly deploy (:50), then gather AC10 live evidence
   (`tools/list` excerpt: one canonical name, one alias with
   `x-deprecated`, a deprecation hint on an alias call) and write the
   final ≤20-line report per the original task's format.
