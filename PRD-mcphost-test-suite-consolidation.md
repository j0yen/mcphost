# PRD — mcphost test suite consolidation: 289 test binaries become a handful, files stay where they are

- Status: building
- Lane: redbaron 2026-09-12T21:21:56Z pid=1585240 boot=c6865fd1-71c2-48cf-818e-5e1f2246b3fe
- iter_log: 2026-09-12T17:55:00Z requeued by operator: mock_justifications line added for deferred AC8 | 2026-09-12T16:56:15Z needs_classification: lint gate fail: deferred_acs=[8] but no mock_justifications: line (prd-lint.sh deferred-acs-missing-justification)
- PM: Joe Yen
- Drafted: 2026-09-12
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_version_bump: patch
- build_priority: normal
- publish: j0yen/private
- test_prefix: suite
- deferred_acs: [8]
- mock_justifications: AC8 — the cross-host parity script from mcphost-tests-host-independence needs a second box and casper is deleted; the name-matching change is proven single-host by AC1's nextest list diff, and cross-host parity is deferred until a burst box exists again.
- Vision: visions/buildloop-operations.md
- Engineering target: extend ~/wintermute/mcphost — `Cargo.toml` (`autotests = false`, `[[test]]` entries), generated `tests/suite_<area>.rs` files that `#[path]`-include the existing `tests/*.rs`, one shared `common`, `scripts/gen-test-suites.sh` with a completeness check, CI hook; no `src/` change

## TL;DR

Cargo stops turning each of mcphost's 289 top-level test files into its own 280 MB binary. A generator writes a few `[[test]]` suite files that include the existing test files by path, so every test keeps its name, its file, and its AC pairing. `common` compiles once instead of 269 times. The gate links about six binaries instead of 289, and `target/` stops holding tens of gigabytes of near-identical executables.

## Problem statement

Joe's mcphost gate takes 21–42 minutes for a 413 s cold compile. The repo's `tests/` directory holds 289 top-level `.rs` files (2026-09-12 count) and cargo's default `autotests` makes each one a separate test binary; each links the whole 52K-LoC crate and its 41 dependencies, and measured binaries are 274–291 MB (`attrib_ac1_loopback_signup_unstamped`, `infer_ac08_ac09_python_requirements`). `target/debug/deps` holds 1,391 such binaries. 269 of the files declare `mod common;`, so the shared helper is compiled 269 times. RedBaron's root disk reached 81–86 % on 09-08, with `target/` dirs the known bulk. Every gate pays the link cost 289 times for one crate; every new AC PRD adds another binary. PRD-build-gate-phase-timing measures the `gate` phase this PRD moves; until it ships, the expected saving is inferred from binary count and size, not measured.

Why-chain: none; not a failure (vision, 2026-09-12 gate-cost section, leaf 1 is the assumption this PRD acts on and the timing PRD verifies).

## Goals

- At most ten test binaries for the whole `tests/` tree; every existing test still runs under its existing name and file.
- `common`, `support`, and `ci_sandbox_support` compile once.
- Adding a new `tests/<name>.rs` without registering it fails CI with the file named.
- The AC-pairing classifier, receipts, and intent cards see the same file names and paths as before.

## Non-goals

- Changing any test's body or assertion, or `src/`.
- Changing the runner: nextest stays, and its per-test process isolation is what keeps host-independence fixes valid.
- Reducing the number of test files. Consolidation is at the binary level only.
- Sharding suites across machines.

## User stories

- As the loop, I want a gate to link a handful of binaries, so that the `gate` phase drops and more PRDs clear per tick.
- As the AC-pairing classifier, I want `tests/<prefix>_acN_*.rs` to exist exactly where it does today, so that verified-completed keeps pairing without a change.
- As a builder adding `tests/newprefix_ac1_thing.rs`, I want the generator to pick it up or CI to tell me it did not, so that a test can never be silently unlinked.
- As the operator watching disk, I want `target/debug/deps` to stop growing by 280 MB per test file, so that RedBaron's root drive stops filling.

## Requirements

P0
- `Cargo.toml`: `autotests = false`; one `[[test]]` per area with `path = "tests/suite_<area>.rs"`; areas chosen by filename prefix so that no suite exceeds roughly 60 files and the total is ≤10.
- `scripts/gen-test-suites.sh`: scans `tests/*.rs` (top-level only), assigns each to an area by prefix table, writes `tests/suite_<area>.rs` with `#[path = "<file>.rs"] mod <ident>;` lines plus one `mod common;` (and `support`, `ci_sandbox_support` where used); output is deterministic and sorted.
- Per-file `mod common;` (and the two other helper declarations) are removed from the included files and replaced by `use crate::common;` or equivalent so the helper compiles once; a test that names `common::` items unchanged still compiles.
- `gen-test-suites.sh --check` exits non-zero naming any top-level `tests/*.rs` not included in a suite, or any suite file that drifted from the generator's output; wired into `ci-checks`.
- Every test that ran before runs after: nextest's test list, with the suite prefix stripped, equals the pre-change list.
- The verified-completed pairing classifier and the intent-card test pointers resolve exactly as before (file paths unchanged).

P1
- `.config/nextest.toml` emits JUnit with per-test timing so the `gate` phase can later be split into test time.
- README section: how to add a test (drop the file, run the generator or let CI tell you).

Non-functional
- Full `cargo nextest run` wall on RedBaron after the change ≤ 50 % of before at the same commit, measured by PRD-build-gate-phase-timing's `gate:` seconds or, until it ships, by two timed runs in the receipt.
- `target/debug/deps` growth per new test file ≤ 5 MB.

## Success metrics

| metric | baseline (2026-09-12) | target | method | timeframe |
|---|---|---|---|---|
| primary: test binaries linked per gate | 289 | ≤10 | `cargo nextest list --message-format json` binary count | at ship |
| secondary: `gate` phase seconds (median over 5 gates) | unknown; whole wall 1258–2544 s | ≤50 % of pre-change | PRD-build-gate-phase-timing journal field | 7 days after both ship |
| secondary: `target/debug/deps` size after one clean build | ~80 GB (289 × ~280 MB) | ≤5 GB | `du -sh` in the receipt | at ship |
| guardrail: tests that ran before and not after | 0 | 0 | nextest list diff | at ship |
| guardrail: AC pairings broken | 0 | 0 | verified-completed run on the archived mcphost PRDs' test pointers | at ship |

## Technical considerations

- `#[path]` modules keep the source files in place; module identifiers are derived from file names (`ac01_unauthenticated_lists_signup` → same, with leading digits handled).
- Tests with identical function names in different files collide only if the suite flattens them; nesting under the file module avoids it.
- Tests that set process-global state (env vars, working directory) already run in separate processes under nextest; consolidation does not change that.
- Area prefix table lives in the generator and in the PRD's open question until ship; expect roughly: `ac`, `hooks`, `tooltest`, `infer`, `attrib`, `signup`/`billing`, `misc`.
- The mcphost-tests-host-independence PRD's parity comparison counted suites (280); after this PRD that count is ≤10, and the parity script's suite matching must key on test names, not binaries. Checked in AC.

## Migration / compatibility

- One patch release; no runtime behavior change.
- The gate baseline and intent cards reference test file paths; unchanged. The reviewer-debt PRD queued on the same repo is serialized by the one-PRD-per-repo rule; whichever lands second rebases trivially because this PRD touches only `Cargo.toml`, `tests/suite_*.rs`, the helper declarations, and `scripts/`.

## Open questions

| question | owner | due |
|---|---|---|
| Final area table (which prefixes share a binary)? Default: the seven above. | Joe | at ship |
| Should the burst-lane parity script match on test names now that suites are few? | Joe | at ship |

## Acceptance criteria

1. P0 — Given the repo at HEAD, When `gen-test-suites.sh` runs and `cargo nextest list` runs, Then at most ten test binaries are listed and the set of test names (suite prefix stripped) equals the set listed at the parent commit.
2. P0 — Given a new file `tests/zzz_ac1_probe.rs` that is not in any suite, When `gen-test-suites.sh --check` runs, Then it exits non-zero naming `tests/zzz_ac1_probe.rs`; after regenerating, it exits 0 and the test is listed.
3. P0 — Given the consolidated layout, When `grep -c 'mod common' tests/*.rs` runs over the included files, Then the count is 0 and each suite file declares `mod common;` once.
4. P0 — Given the archived mcphost PRDs' test pointers (for example `tests/hooks_ac1_*.rs`), When the verified-completed pairing classifier runs against them, Then every pairing that resolved before resolves now.
5. P0 — Given a clean `target/`, When `cargo nextest run` completes, Then `du -s target/debug/deps` is under 5 GB and the receipt records the size.
6. P0 — Given two timed full nextest runs at the same commit, one before and one after the change (or the `gate:` phase from PRD-build-gate-phase-timing when present), When compared in the receipt, Then the after time is ≤50 % of before.
7. P1 — Given `.config/nextest.toml` with JUnit enabled, When the suite runs, Then a JUnit file exists with a duration per test.
8. P1 — Given the parity script from mcphost-tests-host-independence, When run against the new layout on one host, Then it matches by test name and reports the same test count as before.
9. P1 — Given the `suite` selftest cases, When the repo's test selftest runs, Then they are named and green.
