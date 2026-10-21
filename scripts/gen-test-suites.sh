#!/usr/bin/env bash
# gen-test-suites.sh -- PRD-mcphost-test-suite-consolidation.
#
# Cargo's default `autotests` turns every top-level `tests/*.rs` file into
# its own integration-test binary, linking the whole crate (52K LoC, 41
# deps) once per file. At 289 files that was 289 274-291 MB binaries
# (`target/debug/deps` ~80 GB) and 269 redundant compiles of
# `tests/common/mod.rs` (`mod common;` in every one of them). This script
# turns that off (`autotests = false` in Cargo.toml) and writes a handful of
# `tests/suite_<core|sandbox>_NN.rs` files that `#[path]`-include the
# EXISTING test files by name -- every test keeps its file, its name, and
# its AC pairing; only which BINARY it links into changes.
#
# Partitioned by sandbox/core FIRST (scripts/ci-test-partition.sh's
# `classify-file`, the same regex that already routes CI's two jobs), then
# bucketed by filename-prefix AREA within each partition, sorted, capped at
# MAX_PER_SUITE files per suite -- so no suite ever mixes a file that needs
# the userns capability with one that doesn't (mixing would hand one CI
# job's capability grant to a binary that also contains files that must NOT
# have it, silently breaking PRD-mcphost-ci-sandbox-coverage's isolation).
# ci-test-partition.sh's `check` re-derives this same partition and refuses
# to route a suite whose membership disagrees with its own classify().
#
# `common`/`ci_sandbox_support` are declared (`mod common;` /
# `mod ci_sandbox_support;`) exactly ONCE per suite file that needs them
# (i.e. compiled once per suite, not once per member file); every member
# file's own top-level `mod common;` / `mod ci_sandbox_support;` line is
# rewritten in place to `use crate::common;` / `use crate::ci_sandbox_support;`
# -- Rust 2018+ path resolution means `common::Foo` inside a nested
# `#[path]`-included module still resolves once `common` is a module at the
# suite's crate root, so no other line in any test file changes. Any other
# member-owned `#[path = "support/<name>.rs"] mod <name>;` declaration (e.g.
# `support/host.rs`, `support/lanecov.rs`) gets the identical treatment --
# hoisted to the suite root, rewritten in the member file to
# `use crate::<name>;` -- because loading the same on-disk file as a module
# more than once inside one suite binary is exactly what `clippy::duplicate_mod`
# flags (mcphost-test-suite-consolidation reviewer-agent block
# clippy-duplicate-mod-shared-file-multi-suite, tests/lanecov_ac0N_*.rs each
# declaring their own `mod lanecov;` once consolidated into one binary).
# These are the only edits this script makes to a `tests/*.rs` file's
# content; test bodies, assertions, and every other line are untouched and
# keep resolving exactly as before, because `#[path]` module resolution is
# relative to the FILE's own on-disk directory (unchanged: files stay where
# they are), not to how something else `#[path]`-includes that file.
#
# Usage:
#   scripts/gen-test-suites.sh            regenerate tests/suite_*.rs +
#                                          Cargo.toml's [[test]] block, and
#                                          migrate any un-migrated member
#                                          file's mod common;/mod
#                                          ci_sandbox_support; line, in place
#   scripts/gen-test-suites.sh --check    exit non-zero (naming what's wrong)
#                                          if the committed tests/suite_*.rs
#                                          files, Cargo.toml's [[test]] block,
#                                          or any member file's helper-mod
#                                          line has drifted from what this
#                                          script would generate right now,
#                                          OR a top-level tests/*.rs file
#                                          exists that no suite includes.
#                                          Writes nothing.
set -uo pipefail
cd "$(dirname "$0")/.."

python3 - "${1:-}" <<'PY'
import glob
import json
import os
import re
import subprocess
import sys

MODE = sys.argv[1] if len(sys.argv) > 1 else ""
CHECK = MODE == "--check"

REPO_ROOT = os.getcwd()
CLASSIFY_SCRIPT = os.path.join(REPO_ROOT, "scripts", "ci-test-partition.sh")

# The shard count is read from `ci-test-partition.sh shards` (its one
# SANDBOX_SHARDS definition) and enforced at the bottom of this script. If the
# sandbox cap below ever pushes that partition past that many suites (or the core
# partition past however many exist today), widen SANDBOX_SHARDS there; the
# workflow matrix follows it (ci.yml's plan job). P0 requirement: no suite
# exceeds roughly 60 files, and the total (both partitions) stays <=10.
#
# Per-partition, not one global number (2026-09-12 follow-up): the
# exclusive-global singleton fix above (`distribute_exclusive`) adds one
# whole extra binary per exclusive-subscriber file. Originally all such
# files lived in "core" (5 of them) -- at cap 80, core's non-exclusive
# files bucketed into 3 suites, landing the grand total at 3 + 5 + 2 = 10
# (sandbox's 2-suite split is already load-bearing for
# `.github/workflows/ci.yml`'s sandbox matrix -- a 1-suite sandbox
# would starve a later shard and trip ci-test-partition.sh's own empty-shard
# guard -- so its cap stays untouched by this kind of bump).
#
# PRD-mcphost-python-kind-plain-env (2026-09-13 follow-up): a new sandbox
# test (`plainenv_ac09_publish_journal_names_not_values.rs`, asserting on a
# captured tracing subscriber for a publish-audit check) is the first
# exclusive-global file ever classified "sandbox" -- it earns its own
# singleton binary same as core's, pushing sandbox from 2 to 3 (2 normal +
# 1 singleton) and the grand total to 11. Sandbox's cap can't absorb this
# (raising it would still leave the 1 extra singleton binary, and shrinking
# its normal-bucket count below 2 risks the empty-shard guard on
# the shard count the moment sandbox grows again) so core's cap absorbs it
# instead: raised 80 -> 120, collapsing core's non-exclusive buckets from 3
# to 1, landing the grand total at 1 + 6 + (2 + 1) = 10 -- exactly the AC1
# ceiling, not under it by luck. (Core's exclusive-singleton count moved
# 5 -> 6 in the same follow-up; if core ever needs a second normal bucket
# again, this comment's arithmetic needs a fresh look, same as this one did.)
#
# PRD-mcphost-client-attribution-default-leak (2026-09-13 follow-up): by
# this PRD's start, core's non-exclusive area groups had already regrown
# past the 120 cap on their own (organic test growth since the note above
# was written -- that arithmetic was never re-verified per-commit), sitting
# at 2 normal buckets rather than the 1 the note above assumed. This PRD's
# own 2 new regression tests (`attribdefault_ac1_*`/`attribdefault_ac2_*`,
# a brand-new `attribdefault` area group) tipped that 2nd bucket over 120,
# spawning a 3rd core-normal bucket and an 11th suite binary overall --
# caught by `suite_ac1_ten_binaries_and_names_preserved`'s P0 assertion.
# 120 -> 125 was the smallest tested raise that re-collapses core's normal
# buckets to 2, landing the grand total back at 10. Same caveat as above:
# this is a snapshot, not a standing guarantee -- if it needs raising
# again, recount core's non-exclusive files before guessing a new number.
#
# PRD-mcphost-agent-directory (2026-09-13 follow-up): this PRD's own new
# `agentdir` area group (4 files, consolidated per-AC same as
# `compat_ac01_ac02_ac03_cache_fields.rs`'s convention) tipped core's
# normal buckets from 2 to 3 again, spawning an 11th suite binary --
# caught by the same `suite_ac1_ten_binaries_and_names_preserved` P0
# assertion. 125 -> 126 was the smallest tested raise that re-collapses
# core's normal buckets to 2, landing the grand total back at 10.
#
# PRD-mcphost-agent-inbox (2026-09-13 follow-up): this PRD's own new `msg`
# area group (12 files, one per AC, same per-AC consolidation convention)
# tipped core's normal buckets from 2 to 3 again, spawning an 11th suite
# binary -- caught by the same `suite_ac1_ten_binaries_and_names_preserved`
# P0 assertion. 126 -> 130 was the smallest tested raise that re-collapses
# core's normal buckets to 2, landing the grand total back at 10.
#
# PRD-mcphost-agent-wake (2026-09-15 follow-up): this PRD's own new `wake`
# area group (8 files, one per AC, same per-AC consolidation convention)
# tipped core's normal buckets from 2 to 3 again, spawning an 11th suite
# binary -- caught by the same `suite_ac1_ten_binaries_and_names_preserved`
# P0 assertion. 130 -> 135 was the smallest tested raise that re-collapses
# core's normal buckets to 2, landing the grand total back at 10.
#
# PRD-mcphost-agent-consent (2026-09-15 follow-up, rebased onto wake):
# this PRD's own new `consent` area group (2 files, consolidated per-AC-range
# same as `agentdir_ac01_ac02_ac03_ac04_identity_and_lookup.rs`'s convention)
# stacks on top of wake's own bump above -- landing both PRDs' new files
# tipped core's normal buckets from 2 to 3 again, spawning an 11th suite
# binary -- caught by the same `suite_ac1_ten_binaries_and_names_preserved`
# P0 assertion. 135 -> 136 was the smallest tested raise (on top of wake's
# 135) that re-collapses core's normal buckets to 2, landing the grand
# total back at 10.
#
# PRD-mcphost-proof-lane-loop-config (2026-09-17 follow-up, rebased onto
# consent): this PRD's own new `lanecov` area group (6 files, one per AC,
# same per-AC consolidation convention) tipped core's normal buckets from 2
# to 3 again, spawning an 11th suite binary -- caught by the same
# `suite_ac1_ten_binaries_and_names_preserved` P0 assertion. 136 -> 137 was
# the smallest tested raise (on top of consent's 136) that re-collapses
# core's normal buckets to 2, landing the grand total back at 10.
#
# PRD-mcphost-tenant-self-offboard (2026-09-17, rebased onto proof-lane-
# loop-config): this PRD's own 5 new `self_offboard_ac*` files (one per AC,
# same per-AC consolidation convention) tipped core's normal buckets from 2
# to 3 again, spawning an 11th suite binary -- caught by the same
# `suite_ac1_ten_binaries_and_names_preserved` P0 assertion. 137 -> 141 was
# the smallest tested raise (on top of proof-lane-loop-config's 137) that
# re-collapses core's normal buckets to 2, landing the grand total back at 10.
#
# PRD-mcphost-share-a-tool-not-a-key (2026-09-19, rebased onto
# tenant-self-offboard): this PRD's own 8 new `mcphost_share_a_tool_not_a_key_ac*`
# files (one per AC, same per-AC consolidation convention) tipped core's
# normal buckets from 2 to 3 again, spawning an 11th suite binary -- caught
# by the same `suite_ac1_ten_binaries_and_names_preserved` P0 assertion.
# 141 -> 146 was the smallest tested raise (on top of tenant-self-offboard's
# 141) that re-collapses core's normal buckets to 2, landing the grand
# total back at 10.
#
# PRD-mcphost-wasm-kind (2026-09-19, rebased onto share-a-tool-not-a-key):
# this PRD's own 10 new `wasmkind_ac*` files (one per AC, same per-AC
# consolidation convention) tipped core's normal buckets from 2 to 3 again,
# spawning an 11th suite binary -- caught by the same
# `suite_ac1_ten_binaries_and_names_preserved` P0 assertion. 146 -> 150 was
# the smallest tested raise (on top of share-a-tool-not-a-key's 146) that
# re-collapses core's normal buckets to 2, landing the grand total back at 10.
#
# PRD-mcphost-host-tool-deprecation (2026-09-20, rebased onto wasm-kind):
# this PRD's own up to 8 new `mcphost_host_tool_deprecation_ac*` files (one
# per non-deferred AC, same per-AC consolidation convention) tip core's
# normal buckets from 2 to 3 again once more than one of them lands,
# spawning an 11th suite binary -- caught by the same
# `suite_ac1_ten_binaries_and_names_preserved` P0 assertion. 150 -> 158
# (149 pre-existing core-normal files + this PRD's 8 own, plus a 1-file
# margin) was chosen up front to cover the whole PRD's file count in one
# raise rather than one per AC, re-collapsing core's normal buckets to 2
# and landing the grand total back at 10 for every AC in between.
#
# PRD-mcphost-data-retention (2026-09-20, rebased onto host-tool-deprecation):
# this PRD's own new `mcphost_data_retention_ac*` area group (one file per
# AC, same per-AC consolidation convention) tips core's normal buckets from
# 2 to 3 again once enough of them land, spawning an 11th suite binary --
# caught by the same `suite_ac1_ten_binaries_and_names_preserved` P0
# assertion. 158 -> 161 was the smallest tested raise (on top of
# host-tool-deprecation's 158) that re-collapses core's normal buckets to
# 2, landing the grand total back at 10 through its 9th file (AC9). Its
# 10th file (AC7, added after AC9/AC10) tipped it over the same way again;
# 161 -> 162 was the smallest further raise that re-collapses it back to 10.
#
# PRD-mcphost-tenant-data-export (2026-09-20, rebased onto data-retention):
# this PRD's own new `mcphost_tenant_data_export_ac*` files (one per
# non-deferred AC, same per-AC consolidation convention) tipped core's
# normal buckets from 2 to 3 again, spawning an 11th suite binary -- caught
# by the same `suite_ac1_ten_binaries_and_names_preserved` P0 assertion.
# 162 -> 165 was the smallest tested raise (on top of data-retention's 162)
# that re-collapses core's normal buckets to 2, landing the grand total
# back at 10.
#
# PRD-mcphost-checkcompat-port-race (2026-09-20, rebased onto
# tenant-data-export): a different mechanism than every entry above -- this
# PRD's own 6 new `checkcompat_race_ac*` files all fold into the
# pre-existing `checkcompat` area group (its stems share one prefix before
# the first `_`), so core's *normal* bucket count didn't move on its own.
# What moved is the exclusive-singleton count (`is_exclusive_global`):
# `checkcompat_race_ac02_...rs` installs a global tracing subscriber (same
# as `autherr_ac6`/`compat_ac13_ac14`/`sessionkey_ac17`/`synthetic_ac03`/
# `tenant_delete_ac08` already do) to capture wait_ready's "foreign server
# on port" log line, earning its own singleton binary same as theirs --
# core-exclusive 5 -> 6, spawning an 11th suite binary on its own even with
# core's *normal* buckets unchanged at 2. Collapsing those 2 normal buckets
# to 1 (the only lever this cap has) needed a much bigger jump than prior
# entries' +1..+10-file bumps: on top of tenant-data-export's 165 and core's
# own organic growth since, the smallest tested raise that re-collapses
# core's normal buckets to 1 is 327, landing the grand total back at 10
# (1 normal + 6 exclusive + 3 sandbox). AC7's own follow-on file
# (`checkcompat_race_ac07_suite_green_and_clippy_clean.rs`, still in the
# `checkcompat` area group) fits inside this raise's existing margin. AC8's
# follow-on file (`checkcompat_race_ac08_deferral_is_justified.rs`, same
# `checkcompat` area group) also fits inside it.
#
# PRD-mcphost-agent-mesh-ops (2026-09-20, rebased onto checkcompat-port-race):
# this PRD's own new `meshops_ac*` area group (9 files, one per AC, same
# per-AC consolidation convention) tipped core's normal buckets from 1 to 2
# again, spawning an 11th suite binary -- caught by the same
# `suite_ac1_ten_binaries_and_names_preserved` P0 assertion. 327 -> 336 was
# the smallest tested raise that re-collapses core's normal buckets to 1,
# landing the grand total back at 10.
#
# PRD-mcphost-team-memory (2026-09-21, rebased onto agent-mesh-ops, AC1):
# this PRD's own `mcphost_team_memory_ac01_llms_txt_section.rs` (a plain
# `core`-classified string test, no global tracing subscriber -- core's
# exclusive-singleton count stays at 6) joins the pre-existing normal
# bucket, spawning an 11th suite binary -- caught by the same
# `suite_ac1_ten_binaries_and_names_preserved` P0 assertion. 336 -> 337 was
# the smallest tested raise that re-collapses core's normal buckets to 1,
# landing the grand total back at 10.
#
# PRD-mcphost-team-memory (2026-09-22, AC8 follow-up): this PRD's own
# `mcphost_team_memory_ac08_live_test_skipped_without_env.rs` (also
# `core`-classified, also no global tracing subscriber) joins the same
# normal bucket as AC1's file above, spawning an 11th suite binary again --
# caught by the same P0 assertion. 337 -> 338 was the smallest tested
# raise that re-collapses core's normal buckets to 1, landing the grand
# total back at 10. (This PRD's remaining test files are all
# `sandbox`-classified and fit inside sandbox's existing 3-suite split
# without needing this cap raised further.)
#
# PRD-mcphost-database-in-a-minute (2026-09-22, AC1): this PRD's own
# `mcphost_database_in_a_minute_ac01_llms_txt_section.rs` (also
# `core`-classified, no global tracing subscriber) joins the same "mcphost"
# normal bucket, spawning an 11th suite binary again -- caught by the same
# P0 assertion. 338 -> 339 was the smallest tested raise that re-collapses
# core's normal buckets to 1, landing the grand total back at 10.
#
# PRD-mcphost-database-in-a-minute (2026-09-22, AC8 follow-up): this PRD's
# own `mcphost_database_in_a_minute_ac08_live_test_skipped_without_env.rs`
# (also `core`-classified, no global tracing subscriber, no python/sandbox
# surface) joins the same "mcphost" normal bucket, spawning an 11th suite
# binary again -- caught by the same P0 assertion. 339 -> 340 was the
# smallest tested raise that re-collapses core's normal buckets to 1,
# landing the grand total back at 10.
#
# PRD-mcphost-uptime-probes (2026-09-22, rebased onto database-in-a-minute,
# AC1): this PRD's own `mcphost_uptime_probes_ac01_llms_txt_section.rs`
# (also `core`-classified, also no global tracing subscriber) joins the
# same "mcphost" normal bucket, spawning an 11th suite binary again --
# caught by the same P0 assertion. 340 -> 341 was the smallest tested
# raise that re-collapses core's normal buckets to 1, landing the grand
# total back at 10.
#
# PRD-mcphost-uptime-probes (2026-09-22, AC8 follow-up): this PRD's own
# `mcphost_uptime_probes_ac08_live_test_skipped_without_env.rs` (also
# `core`-classified, also no global tracing subscriber) joins the same
# normal bucket as AC1's file above, spawning an 11th suite binary again --
# caught by the same P0 assertion. 341 -> 342 was the smallest tested
# raise that re-collapses core's normal buckets to 1, landing the grand
# total back at 10. (This PRD's remaining test files are all
# `sandbox`-classified and fit inside sandbox's existing 3-suite split
# without needing this cap raised further.)
#
# PRD-mcphost-agent-channels (2026-09-22): this PRD's own new
# `channel_ac*` area group (10 files, one per AC, same per-AC
# consolidation convention) tipped core's normal buckets from 1 to 2
# again, spawning an 11th suite binary -- caught by the same
# `suite_ac1_ten_binaries_and_names_preserved` P0 assertion, rebased onto
# the 342 -> 350 raise above; re-tuned post-rebase to re-collapse core's
# normal buckets to 1, landing the grand total back at 10.
#
# PRD-mcphost-webhook-inbox (2026-09-22, rebased onto agent-channels): this
# PRD's own eleven `mcphost_webhook_inbox_ac*.rs` files (also
# `core`-classified, also no global tracing subscriber) join the same
# normal bucket, spawning an 11th suite binary again -- caught by the same
# P0 assertion. 360 -> 375 is the smallest tested raise that re-collapses
# core's normal buckets to 1, landing the grand total back at 10.
#
# PRD-mcphost-signup-kill-switch-and-source (2026-09-23): this PRD's own
# ten `mcphost_signup_kill_switch_and_source_ac*.rs` files (also
# `core`-classified, also no global tracing subscriber) join the same
# normal bucket, spawning an 11th suite binary again -- caught by the same
# P0 assertion. 375 -> 395 is the smallest tested raise that re-collapses
# core's normal buckets to 1, landing the grand total back at 10.
#
# PRD-mcphost-sandbox-egress-allowlist (2026-09-23): this PRD's own new
# `mcphost_sandbox_egress_allowlist_ac*` area group (10 files, one per AC,
# same per-AC consolidation convention) is mostly `sandbox`-classified
# (every file that publishes/calls through `python_kind_registry`) --
# unlike every prior follow-up above, this one tipped SANDBOX's bucketing
# over, not core's, spawning a 4th sandbox suite binary (11th overall) --
# caught by the same `suite_ac1_ten_binaries_and_names_preserved` P0
# assertion. 60 -> 62 is the smallest tested raise that re-collapses
# sandbox's buckets to 3, landing the grand total back at 10.
#
# PRD-mcphost-human-claim-magic-link (2026-09-23, rebased onto
# sandbox-egress-allowlist): this PRD's own thirteen
# `mcphost_human_claim_magic_link_ac*.rs` files (also `core`-classified,
# also no global tracing subscriber) join the same normal bucket, spawning
# an 11th suite binary again -- caught by the same P0 assertion. 395 -> 440
# is the smallest tested raise that re-collapses core's normal buckets to
# 1, landing the grand total back at 10. sandbox's own 60 -> 62 raise above
# is independent (a different bucket) and stands unchanged.
#
# PRD-mcphost-oauth-resource-server (2026-09-25): this PRD's own ten
# `oauthrs_ac*.rs` files (also `core`-classified, also no global tracing
# subscriber) join the same normal bucket, spawning an 11th suite binary
# again -- caught by the same P0 assertion. 440 -> 444 is the smallest
# tested raise that re-collapses core's normal buckets to 1, landing the
# grand total back at 10.
#
# PRD-mcphost-table-semantic-model (2026-09-25): this PRD's own ten
# `tablemodel_ac*.rs` files (also `core`-classified, also no global tracing
# subscriber) join the same normal bucket, spawning an 11th suite binary
# again -- caught by the same P0 assertion. The 444 -> 448 raise landed
# alongside the AC7-10 commits was not actually re-checked against
# --check afterward and undershot; 448 -> 454 is the smallest tested raise
# that reproduces the already-committed suite files byte-for-byte and
# re-collapses core's normal buckets to 1, landing the grand total back
# at 10.
#
# PRD-mcphost-sqlite-busy-timeout-audit (2026-09-25, rebased onto
# table-semantic-model): this PRD's own ten `busyaudit_ac*.rs` files plus
# tests/support/busyaudit.rs (also `core`-classified) join the same normal
# bucket on top of table-semantic-model's own raise above, spawning an
# 11th suite binary again -- caught by the same P0 assertion. 454 -> 464 is
# the smallest tested raise that re-collapses core's normal buckets to 1,
# landing the grand total back at 10.
#
# PRD-mcphost-alerting-webhook (2026-09-25, rebased onto
# mcphost-sqlite-busy-timeout-audit): this PRD's own eleven `alert_ac*.rs`
# files (also `core`-classified) join the same normal bucket on top of
# busyaudit's own raise above, spilling core past the 464 cap again --
# caught by the same P0 assertion. 464 -> 475 is the smallest tested raise
# that re-collapses core's normal buckets back to 1, landing the grand
# total back at 10.
#
# PRD-mcphost-docs-semantic-search (2026-09-25, rebased onto
# mcphost-alerting-webhook): this PRD's own eleven `docsearch_ac*.rs`
# files (also `core`-classified, also no global tracing subscriber) join
# the same normal bucket, spawning an 11th suite binary again -- caught by
# the same P0 assertion. Re-tuned post-rebase (onto d273490, which had
# already independently raised the cap past this PRD's original 475-based
# arithmetic) to re-collapse core's normal buckets to 1 against the full
# rebased file set, landing the grand total back at 10.
#
# mcphost-docs-semantic-search): this PRD's own eleven `enduser_ac*.rs`
# files (also `core`-classified) join the same normal bucket on top of
# docs-semantic-search's own 560 cap above, spawning an 11th binary again.
# 560 -> 576 re-collapses core's normal buckets to 1, landing the grand
# total back at 10, with a little headroom for the rest of this PRD's own
# remaining AC files.
#
# PRD-mcphost-oauth-conformance-harness (2026-09-26): this PRD's own
# seven `oauthconf_ac*.rs` files (also `core`-classified) join the same
# normal bucket on top of enduser's own 576 cap above, spawning an 11th
# suite binary again -- caught by the same P0 assertion. 576 -> 600
# re-collapses core's normal buckets to 1, landing the grand total back
# at 10.
#
# PRD-mcphost-oauth-client-policy: this PRD's own nine `oauthpol_ac*.rs`
# files spilled core's last bucket into an 11th binary again. 600 -> 620
# re-collapses it to 1, landing the grand total back at 10.
#
# PRD-mcphost-tenant-resource-metadata (2026-09-27): this PRD's own
# `tenantprm_ac*.rs` files (also `core`-classified) join the same normal
# bucket on top of oauth-conformance-harness's own 600 cap above, spawning
# an 11th suite binary again -- caught by the same P0 assertion. 600 -> 620
# re-collapses core's normal buckets to 1, landing the grand total back
# at 10.
#
# Rebasing mcphost-oauth-client-policy onto tenant-resource-metadata
# (2026-09-27): both PRDs above independently raised 600 -> 620 on top of
# the same oauth-conformance-harness baseline, not knowing about each
# other; landing both together stacks oauthpol's nine files AND
# tenantprm's files on top of the same 600 starting point. See below for
# whatever cap this rebase's own P0 assertion run required.

# PRD-mcphost-federated-end-user-login (2026-09-27): this PRD's own ten
# `fedlogin_ac*.rs` files (also `core`-classified, also no global tracing
# subscriber) join the same normal bucket on top of tenant-resource-metadata's
# own 620 cap above, spawning an 11th suite binary again -- caught by the
# same `suite_ac1_ten_binaries_and_names_preserved` P0 assertion. 620 -> 630
# re-collapses core's normal buckets to 1, landing the grand total back
# at 10.
#
# Rebasing mcphost-oauth-demand-signal onto federated-end-user-login
# (2026-09-27): this PRD's own `oauthsig_ac*.rs` files (also `core`-classified)
# independently raised 620 -> 640 on top of the same tenant-resource-metadata
# baseline, not knowing about fedlogin's own 620 -> 630 bump above; landing
# both together stacks fedlogin's ten files AND oauthsig's files on top of
# the same 620 starting point. 620 -> 650 (620 + fedlogin's +10 + oauthsig's
# +20) re-collapses core's normal buckets to 1, landing the grand total back
# at 10.
#
# Rebasing mcphost-tool-scopes-and-consent onto main after
# mcphost-oauth-demand-signal (#64) merged (2026-09-27): this PRD's own
# `toolscope_ac*.rs` files (also `core`-classified) join the same normal
# bucket on top of oauth-demand-signal's own 650 cap above, spawning an 11th
# suite binary again -- caught by the same P0 assertion. See below for
# whatever cap this rebase's own P0 assertion run required.
#
# 2026-09-27: rebasing mcphost-enterprise-managed-auth onto
# mcphost-tool-scopes-and-consent (#66) stacks this PRD's own `xaa_ac*.rs`
# core-classified files on top of tool-scopes-and-consent's 650 cap above,
# spawning an 11th suite binary again -- caught by the same P0 assertion.
# 650 -> 670 re-collapses core's normal buckets to 1, landing the grand total
# back at 10.

# PRD-mcphost-oauth-unverified-client-consent-warning (rebased onto
# mcphost-tenant-key-missing-is-invalid-params, 2026-09-29 land): this
# PRD's own nine `mcphost_oauth_unverified_client_consent_warning_ac*.rs`
# files (also `core`-classified) join the same normal bucket on top of
# enterprise-managed-auth's own 670 cap above, spawning an 11th suite
# binary again -- caught by the same P0 assertion. 670 -> 680
# re-collapses core's normal buckets to 1, landing the grand total back
# at 10.
#
# PRD-mcphost-session-bound-tenant-after-signup (2026-09-30, fixing
# verifier findings for AC1/AC2/AC4-AC9/AC11/AC12): this PRD's own ten
# `sessbind_ac*.rs` files (also `core`-classified) join the same normal
# bucket on top of the oauth-unverified-client-consent-warning PRD's own
# 680 cap above, spawning an 11th suite binary again -- caught by the same
# P0 assertion. 680 -> 700 re-collapses core's normal buckets to 1,
# landing the grand total back at 10.
#
# PRD-mcphost-run-budget-governor (2026-09-30): this PRD's own nine
# `budget_ac*.rs` core-classified files (AC1-AC9; AC11 is
# sandbox-classified, joining `suite_sandbox_02.rs` instead) join the same
# normal bucket on top of the session-bound-tenant PRD's own 700 cap above,
# spawning an 11th suite binary again -- caught by the same P0 assertion.
# 700 -> 720 re-collapses core's normal buckets to 1, landing the grand
# total back at 10.
#
# PRD-mcphost-chart-in-a-minute (2026-09-30): this PRD's own thirteen
# `chart_ac*.rs` core-classified files (AC11's own sandboxed python-bridge
# test is `sandbox`-classified, unaffected) join the same normal bucket on
# top of the session-bound-tenant-after-signup PRD's own 700 cap above,
# spawning an 11th suite binary again -- caught by the same P0 assertion.
# 700 -> 715 re-collapses core's normal buckets to 1, landing the grand
# total back at 10.
#
# PRD-mcphost-lineage-blast-radius (rebased onto chart-in-a-minute,
# 2026-09-30): this PRD's own eleven `lineage_ac*.rs` files (8 core + 4
# sandbox) join the same normal buckets on top of chart-in-a-minute's own
# 715/90 caps above, spawning an 11th suite binary again -- caught by the
# same P0 assertion. 715 -> 765 (core) and 90 -> 130 (sandbox) re-collapse
# both back to 1 extra bucket each, landing the grand total back at 10.
#
# PRD-mcphost-docs-hybrid-search (rebased onto lineage-blast-radius,
# 2026-09-30): this PRD's own nine `hybrid_ac*.rs` files (also
# `core`-classified) join the same normal bucket on top of
# lineage-blast-radius's own 765 cap above, spawning an 11th suite binary
# again -- caught by the same P0 assertion. 765 -> 785 re-collapses core's
# normal buckets to 1, landing the grand total back at 10.
#
# Rebasing mcphost-chain-run-lineage onto mcphost-spec-unknown-field-
# rejection (2026-10-02, this rebase): this PRD's own fourteen
# `mcphost_chain_run_lineage_ac*.rs` files (also `core`-classified)
# originally raised 680 -> 720 against the session-bound-tenant-after-signup
# baseline; landing them on top of docs-hybrid-search's own 785 cap above
# (chart-in-a-minute, lineage-blast-radius, and docs-hybrid-search all
# landed on main in between and are not files this branch carries) spawns
# an 11th suite binary again -- caught by the same P0 assertion. The exact
# new cap is re-derived below by actually running gen-test-suites.sh
# --check against this rebase's full file set, same as every entry above.
#
# PRD-mcphost-table-concept-graph (rebased onto mcphost-chain-run-lineage,
# 2026-10-02): this PRD's own nine `tgraph_ac*.rs` core-classified files
# (AC9's own sandboxed-tool test is `sandbox`-classified, unaffected) join
# the same normal bucket on top of chain-run-lineage's own 785 cap above,
# spawning an 11th suite binary again -- caught by the same P0 assertion.
# 785 -> 794 (core) re-collapses core's normal buckets to 1, landing the
# grand total back at 10; sandbox's 130 cap already covers tgraph's own
# single sandbox file.
#
# PRD-mcphost-public-tool-url (rebased onto main at 074d6933 / v0.76.0,
# 2026-10-03, run 354): this PRD's own nine `pubtool_ac*.rs` core-classified
# files join the same normal bucket on top of table-concept-graph's own 794
# cap above, spawning an 11th suite binary again -- caught by the same P0
# assertion. 794 -> 810 (core) re-collapses core's normal buckets to 1,
# landing the grand total back at 10.
#
# Pre-existing drift found while landing PRD-mcphost-table-context-and-
# sql-passthrough (rebased onto mcphost-table-concept-graph, 2026-10-02):
# some prior PRD (commit aedb4e5, mcphost-docs-qa-recipe) had already
# pushed sandbox's own normal bucket past its 130 cap above into a 3rd
# sandbox suite binary (11th overall) without re-tuning this cap -- caught,
# on this branch, by the same P0 assertion. 130 -> 170 re-collapses
# sandbox's normal buckets to 1 (2 total: the one `exclusive` singleton
# plus this one), landing the grand total back at 10. Unrelated to this
# PRD's own files (net -4 core-classified files, 0 sandbox-classified: it
# deletes five sandbox-classified predecessor tests and adds one
# core-classified one).
#
# Rebasing mcphost-table-context-and-sql-passthrough onto
# mcphost-public-tool-url (2026-10-03, this rebase): both PRDs above
# independently raised this cap from the same table-concept-graph baseline
# -- public-tool-url bumped core 794 -> 810, table-context-and-sql-
# passthrough bumped sandbox 130 -> 170. Landing both together needs both
# bumps; the exact combined numbers are re-derived below by actually
# running gen-test-suites.sh --check against this rebase's full file set.
#
# PRD-mcphost-dry-run-side-effects (2026-10-03, run 352): this PRD's own
# seven `dryrun_ac0N_*.rs` sandbox-classified files (dry-run writes must
# exercise the real sandboxed write path to prove the rollback) tipped
# sandbox's normal bucket past its 170 cap above into a 3rd sandbox suite
# binary (11th overall) -- caught by the same P0 assertion. 170 -> 175 was
# the smallest tested raise that re-collapses sandbox's normal buckets to
# 1 (2 total: the one `exclusive` singleton plus this one), landing the
# grand total back at 10.
#
# PRD-mcphost-event-trigger-self-test (rebased onto main at 0b56ac76 /
# v0.77.0, 2026-10-03, run 351): this PRD's own twelve
# `mcphost_event_trigger_self_test_ac*.rs` files (also `core`-classified)
# join the same normal bucket on top of public-tool-url's own 810 cap
# above, spawning an 11th suite binary again -- caught by the same P0
# assertion. 810 -> 822 (core) re-collapses core's normal buckets to 1,
# landing the grand total back at 10.
#
# Rebasing mcphost-drift-review onto mcphost-dry-run-side-effects
# (2026-10-03, this rebase): both PRDs above independently raised the
# sandbox cap from the same 170 baseline -- dry-run-side-effects bumped it
# to 175, drift-review bumped it to 180. Landing both together needs a cap
# that covers both PRDs' sandbox files at once; re-derived below by
# actually running gen-test-suites.sh --check against this rebase's full
# file set.
# PRD-mcphost-result-handles (hand-rebased onto main at 84a09d11 / v0.78.0
# after run 368's stale release commit was dropped, 2026-10-03): this PRD's
# own ten `handle_ac*.rs` core-classified files join the same normal bucket
# on top of public-tool-url's own 810 cap, independently bumping core
# 810 -> 870 (same reasoning as this PRD's own prior note below). This
# PRD's own rebase never touched sandbox, so table-context-and-sql-
# passthrough's 130 -> 170 sandbox bump (already landed on main) carries
# forward unchanged. Combined: core takes the larger of the two
# independent bumps (870), sandbox keeps main's landed value (170).
#
# PRD-mcphost-result-handles (rebased onto mcphost-public-tool-url / main at
# a7b9f34c v0.77.0, 2026-10-03, run 368): this PRD's own ten `handle_ac*.rs`
# core-classified files (AC9's own sandboxed python-bridge test is
# sandbox-classified, unaffected; AC10 is deferred, no test file) join the
# same normal bucket on top of public-tool-url's own 810 cap above,
# spawning an 11th suite binary again -- caught by the same P0 assertion.
# 810 -> 870 gives headroom past this PRD's own +10 rather than re-tuning
# to the exact count, so the next PRD's ordinary handful of new tests
# doesn't immediately re-trip the same P0 assertion.
#
# Rebasing mcphost-result-handles onto mcphost-table-context-and-sql-
# passthrough (2026-10-03, this rebase): both PRDs above independently
# raised this cap from the same public-tool-url/table-concept-graph
# baseline -- dry-run-side-effects bumped sandbox 170 -> 175,
# result-handles bumped core 810 -> 870. Landing both together needs both
# bumps; re-verified below by actually running gen-test-suites.sh --check
# against this rebase's full file set.
# Rebasing mcphost-result-handles onto main at run-368-prep time (2026-10-03,
# this hand-prep): main (via drift-review) independently raised sandbox to 180
# while this branch independently raised core to 870. Per the land-conflict rule
# (take the larger cap on each key), the merged caps are core=870 (this branch's
# bump, larger than main's 810) and sandbox=180 (main's bump, larger than this
# branch's 175).
#
# Rebasing mcphost-row-policy onto mcphost-result-handles (2026-10-04, this
# rebase, run 353): this PRD's own twelve `rowpol_ac*.rs` files (11 core + 1
# sandbox, AC12's own sandboxed python-bridge test) join the same normal
# buckets on top of result-handles' own 870/180 caps above -- the exact
# combined numbers are re-derived below by actually running
# gen-test-suites.sh --check against this rebase's full file set.
#
# Rebasing mcphost-event-trigger-self-test onto mcphost-row-policy
# (2026-10-04, this rebase): this PRD's own twelve
# `mcphost_event_trigger_self_test_ac*.rs` files (also `core`-classified)
# originally raised 810 -> 834 against the public-tool-url/dry-run-side-
# effects baseline; landing them on top of row-policy's own 870/180 caps
# above (result-handles and row-policy both landed on main in between and
# are not files this branch carries) spawns an 11th suite binary again --
# caught by the same P0 assertion. The exact new cap is re-derived below
# by actually running gen-test-suites.sh --check against this rebase's
# full file set, same as every entry above.
# PRD-mcphost-uptime-probe-recipe-green (2026-10-10 land, operator): the
# rebase onto v0.107.9 put 1319 files on disk and the sandbox partition over
# 200 per suite -> suite_sandbox_03 appeared (11 core+sandbox binaries) and
# suite_ac1_ten_binaries_and_names_preserved blocked the land's delta gate.
# 200 -> 220 is the smallest tested raise that keeps 2 sandbox suites
# (8 core + 2 sandbox = 10) with --check ok.
# 2026-10-18 (the second core shard over the 720 s budget on main, runs 39129276291,
# 39133346672, 39133722354): suite_core_07 measured 586-735 s and suite_core_08
# 427-711 s on a hosted runner against recorded 341/375 s, so two normal core
# buckets could never fit three 540 s shards (their measured sum alone was
# 1.4-1.7 ks). Two changes: ctrs_ac06 (330-423 s by itself) became a solo-suite
# singleton (SOLO_RE above), and the normal core buckets went from 2 to 3 at
# cap 700 (local per-test timings, RUSTC_BOOTSTRAP=1 `--report-time`, scaled to
# the CI totals, put the three buckets at ~290/370/360 s where the old split was
# ~310/710). 7 singletons + 3 normal + 2 sandbox = 12 = MAX_SUITES.
# 2026-10-18 (main's first 5-shard run 39156878110): suite_core_10 alone measured
# 532 s against CORE_SHARD_PREDICT_LIMIT 540 (recorded 428), and a single suite
# cannot be split by sharding. Its 399 files are the heaviest per file (~1.33 s vs
# ~0.5 s for core_08/09), so one more normal core bucket (cap 700 -> 505) cuts the
# tail across two buckets: 4 normal core buckets of ~502/500/503/269 files, which the
# per-file seconds of the files' previous suites put at ~235/253/366/369 s.
# 7 singletons + 4 normal + 2 sandbox = 13 = MAX_SUITES.
# 2026-10-20 (the first of three sandbox shards over the 300 s run wall on main 39417934601 and PR runs:
# suite_sandbox_02 alone measured 302-316 s against recorded 152 s, its 320 files having
# auto-grown past the 220 cap because a third sandbox bucket would exceed MAX_SUITES): the
# sandbox cap 220 -> 110 splits it into three normal buckets (~119/114/91 files, ~109/107/87 s
# scaled from the refreshed 302 s) and SANDBOX_SHARDS 3 -> 4 gives each its own shard.
# 7 singletons + 4 normal core + 1 singleton + 3 normal sandbox = 15 = MAX_SUITES.
MAX_PER_SUITE = {"core": 505, "sandbox": 110}

# The P0 ceiling on core+sandbox test binaries (tests/suite_ac1_ten_binaries_
# and_names_preserved.rs). ci-test-partition.sh's `check` greps this very line
# (`^MAX_SUITES = N`) so the gate enforces the same cap -- change it here only. When the planned
# core+sandbox count would exceed it, compute_plan() grows the per-suite file
# budget of a partition (sandbox first, else core) to the SMALLEST cap that
# actually removes a bucket, until the plan fits -- the automated form of the
# hand-retunes recorded above (PRD-mcphost-first-tool-from-description, run
# 764, 2026-10-12: 8 core + 3 sandbox = 11 until the sandbox budget auto-grew
# past 220). A partition whose bucket count cannot shrink any further (its
# remaining buckets are exclusive singletons, or it sits at the sandbox shard
# floor) is skipped so the other one is grown instead (run 952, 2026-10-18:
# sandbox = 1 normal + 1 singleton could never shrink, the old loop kept
# growing it to cap 5220 and never touched core, so an 11th suite shipped).
# If no partition can shrink and the plan still exceeds MAX_SUITES the
# generator FAILS (never emits a suite over the cap).
# 10 -> 12 on 2026-10-18 for the solo ctrs_ac06 singleton and the third normal
# core bucket (see MAX_PER_SUITE above); the 10 had been reached with 6 exclusive
# singletons + 2 normal core buckets + 2 sandbox suites, so no core bucket could
# be added without it. tests/suite_ac1_ten_binaries_and_names_preserved.rs and
# tests/suitecap_ac01_*.rs read this very line (no copy of the number).
# 12 -> 13 on 2026-10-18 for the fourth normal core bucket (see MAX_PER_SUITE above):
# 7 exclusive singletons + 4 normal core buckets + 2 sandbox suites.
# 13 -> 15 on 2026-10-20 for the second and third normal sandbox buckets (see MAX_PER_SUITE above).
MAX_SUITES = 15

# scripts/ci-suite-weights.json is generated with the suites: its keys are
# EXACTLY the generated suite names (plus `lib`, the --lib run), so a new suite
# can never lack a weight and a removed suite leaves no stale entry. Measured
# seconds already recorded are kept; a new or resized suite gets a derived
# default (file count x measured seconds-per-file), which a later refresh from
# a green CI run's `finished in` lines replaces.
WEIGHTS_REL = "scripts/ci-suite-weights.json"


GEN_MARK_BEGIN = "# BEGIN gen-test-suites.sh generated suites -- do not edit by hand"
GEN_MARK_END = "# END gen-test-suites.sh generated suites"

SUITE_HEADER_TMPL = """\
// GENERATED by scripts/gen-test-suites.sh -- do not edit by hand.
// Regenerate with: scripts/gen-test-suites.sh
// Check for drift with: scripts/gen-test-suites.sh --check
//
// This file exists only to give cargo one test BINARY for the files listed
// below; it declares no tests of its own. Every included file keeps its
// original name, path, and test names (nextest's test list, with this
// suite's prefix stripped, is unchanged from before consolidation).
"""


def die(msg, code=1):
    print(f"gen-test-suites.sh: {msg}", file=sys.stderr)
    sys.exit(code)


# path -> verdict, filled by classify_all() with ONE classify-files call (was
# one ci-test-partition.sh process per tests/*.rs file: 2220 spawns, ~18 s).
_CLASSIFIED = {}


def classify_all(paths):
    paths = list(paths)
    if not paths:
        return
    out = subprocess.run(
        [CLASSIFY_SCRIPT, "classify-files", *paths],
        capture_output=True, text=True, check=True,
    )
    verdicts = out.stdout.split()
    if len(verdicts) != len(paths):
        die(f"ci-test-partition.sh classify-files returned {len(verdicts)} verdicts for {len(paths)} paths")
    for path, verdict in zip(paths, verdicts):
        if verdict not in ("sandbox", "core"):
            die(f"ci-test-partition.sh classify-files {path!r} returned {verdict!r}")
        _CLASSIFIED[path] = verdict


def classify(path):
    if path in _CLASSIFIED:
        return _CLASSIFIED[path]
    out = subprocess.run(
        [CLASSIFY_SCRIPT, "classify-file", path],
        capture_output=True, text=True, check=True,
    )
    verdict = out.stdout.strip()
    if verdict not in ("sandbox", "core"):
        die(f"ci-test-partition.sh classify-file {path!r} returned {verdict!r}")
    return verdict


# Matches ONLY this generator's own output file names (tests/suite_core_01.rs,
# tests/suite_sandbox_02.rs, tests/suite_env_core_01.rs,
# tests/suite_env_sandbox_01.rs, ...) -- deliberately narrower than "starts
# with suite_", because this PRD's own `test_prefix: suite` AC-pairing
# selftests are named tests/suite_ac<N>_*.rs (verified-completed.sh's
# `prefix:<p>` rule needs that exact shape) and must never be mistaken for
# generated output, or a regen would silently delete them as "stray".
GENERATED_SUITE_RE = re.compile(r'^suite_(?:core|sandbox|env_core|env_sandbox|contracts_core|contracts_sandbox|cargo_core|cargo_sandbox)_[0-9]+\.rs$')

AC_PREFIX_RE = re.compile(r'^(ac[0-9]+)_')
PREFIX_RE = re.compile(r'^([a-z0-9]+)_')

# PRD-mcphost-test-suite-consolidation follow-up (2026-09-12, CI red on
# `cargo test`): several test files each independently install a PROCESS
# global via `tracing::subscriber::set_global_default`, each written under
# the pre-consolidation invariant "this is the only test in the BINARY" (a
# file's own doc comment says exactly that) -- true when one file was one
# binary, false the moment two such files share a suite. `nextest` never hit
# this (it isolates every test into its own process regardless of binary
# membership), which is why the gate's nextest-based verification stayed
# green while CI's plain `cargo test` (no per-test process isolation; tests
# in one binary run as threads in one process) went red on exactly this
# collision. Detected files are spread one-per-bucket (round-robin over the
# buckets the normal prefix bucketing already produced, adding one more
# bucket only for whatever doesn't fit) so at most one such file ever shares
# a binary with itself -- i.e. still zero of them -- without inflating the
# suite count past what P0's <=10-binaries budget allows.
EXCLUSIVE_GLOBAL_RE = re.compile(r'tracing::subscriber::set_global_default')


def is_exclusive_global(content):
    return bool(EXCLUSIVE_GLOBAL_RE.search(content))


# PRD-mcphost-test-suite-flake-lints requirement 5: a file that mutates a
# process-wide env var through `tests/common::EnvGuard` (directly, or via
# the `AdvisoryModeGuard` thin alias) is routed into its own `suite_env_
# <core|sandbox>_NN.rs` binary, segregated from every file that does NOT
# touch env state -- so a future `EnvGuard` bug (the mutex never acquired,
# the restore skipped on panic, ...) can only ever race ANOTHER env-guarded
# test, never silently corrupt an unrelated test's env read. Still split by
# capability (`core`/`sandbox`) first, same as every other bucket here, so a
# sandbox-needing env-guarded file keeps the userns grant its own suite
# binary needs. Matches the thin `AdvisoryModeGuard` alias too, since a
# caller of it (e.g. `common::AdvisoryModeGuard::set("warn")`) never spells
# `EnvGuard` in its own source at all.
ENV_GUARD_RE = re.compile(r'EnvGuard|AdvisoryModeGuard')


def is_env_guarded(content):
    return bool(ENV_GUARD_RE.search(content))


# PRD-mcphost-contract-suite-generated: a test file declares itself a CONTRACT
# test (it asserts a set the repository derives from itself -- the CHANGELOG
# headings, the migration count, the suite partition, the live `tools/list`
# shape) with a `//! contract: <what it asserts>` line in its first 10 lines.
# Marked files are routed into `suite_contracts_<core|sandbox>_NN.rs`
# (capability-partitioned like every other suite), which scripts/pin-tests.sh
# derives its run list from, so the inner, the gate and CI run one set.
CONTRACT_RE = re.compile(r'^//!\s*contract:')


def is_contract(content):
    return any(CONTRACT_RE.match(l) for l in content.split("\n")[:10])


# PRD-mcphost-contract-suite-registry-axis-b requirement 5: two kinds of test
# cannot share a process. One that RE-RUNS cargo in the shared target/ (spawns
# `cargo test ... --list`, which rebuilds and re-uplifts target/debug/mcphost by
# unlink-then-hard-link) and one that EXECS the built binary through
# `env!("CARGO_BIN_EXE_...")`: the exec'd path is missing for a moment, so the
# second dies `NotFound` (CI probe 39163232421: `exmf_ac01/ac02` inside the
# window of `ctrs_ac02`'s cargo). Both are derivable from the file's own source,
# so a file that spawns cargo (unless it is already a singleton binary, like a
# `solo-suite`) is routed into `suite_cargo_<core|sandbox>_NN.rs` and `--check` fails any generated binary that holds both kinds.
SPAWNS_CARGO_RE = re.compile(r'Command::new\(\s*(?:env!\("CARGO"\)|"cargo")\s*\)')
EXECS_BIN_RE = re.compile(r'CARGO_BIN_EXE_')


def code_lines(content):
    """The file with `//` comment lines (docs included) removed: a doc that
    mentions `CARGO_BIN_EXE_` neither execs nor spawns anything."""
    return "\n".join(l for l in content.split("\n") if not l.lstrip().startswith("//"))


def spawns_cargo(content):
    return bool(SPAWNS_CARGO_RE.search(code_lines(content)))


def execs_bin(content):
    return bool(EXECS_BIN_RE.search(code_lines(content)))


# A single test that alone runs for minutes (ctrs_ac06 re-runs every contract
# suite through scripts/pin-tests.sh: 330-423 s on a hosted runner, 10-18 runs
# 39129276291/39133346672/39133722354) cannot be balanced by file count: parked
# inside a normal bucket it makes that bucket the heaviest core shard no matter
# how the other buckets are cut (that shard ran 736 s against the 720 s
# budget with that one test inside suite_core_07). Such a file declares
# `//! solo-suite: <seconds> <why>` in its first 10 lines and is routed into its
# own singleton binary, exactly like an exclusive-global file; the declared
# seconds (the max measured on CI) are that suite's weight in
# scripts/ci-suite-weights.json, so the weight is GENERATED from the marker and
# `--check` fails when the two disagree.
SOLO_RE = re.compile(r'^//!\s*solo-suite:\s*([0-9]+)\b')
SOLO_WEIGHTS = {}  # stem -> declared seconds, filled by compute_plan() from file content


def solo_weight(content):
    for line in content.split("\n")[:10]:
        m = SOLO_RE.match(line)
        if m:
            return int(m.group(1))
    return None


def distribute_exclusive(buckets, exclusive_stems):
    """Give each of `exclusive_stems` its own singleton bucket, prepended
    ahead of the normal buckets. Merging one into an existing (60-file)
    bucket only prevents two exclusive-subscriber files from racing EACH
    OTHER -- it does nothing about the other ~60 unrelated files in that
    same bucket racing the exclusive one on `cargo test`'s default
    thread-per-test concurrency, which still pollutes its captured-log
    buffer with a concurrent, unrelated request's own log line (observed:
    compat_ac13's capture caught a concurrently-running signup test's
    `/mcp` line instead of its own refusal). A singleton bucket is a real
    binary with no other test in it -- the exact isolation the file's own
    pre-consolidation invariant ("only test in the BINARY") relied on,
    restored at the binary-selection level without touching any test
    body."""
    return [[stem] for stem in sorted(exclusive_stems)] + [list(b) for b in buckets]


# Affinity groups: tests/hzkeys_ac02_handler_body_equals_const.rs asserts that
# EVERY test file reading the anonymous /healthz key set is `mod <stem>;`'d in
# its OWN suite file (its meta-test re-runs that one binary with an extra key
# armed and needs all the readers inside it). Which files those are is a
# property of file CONTENT, not of filename prefix (ac14_*, busyaudit_*,
# ctrv_*, hzkeys_*, hzver_*, minimal_* ...), so prefix bucketing alone splits
# them whenever a bucket boundary (a cap fill that shifts with every added
# test file) falls between two of the prefixes -- main went red on --check and
# the PR branches that regenerated to fix it went red on hzkeys_ac02. The
# needles below are the SAME strings that test greps for; keep them in sync
# (tests/hzkeys_ac02_handler_body_equals_const.rs `anon_body_readers`). A file
# matching one is filed under the single area key AFFINITY_KEY, which
# `bucketize` never splits; `check_affinity` then refuses a plan that still
# puts the group in more than one suite (e.g. a reader that is also
# env-guarded, a contract file, or sandbox-classified).
AFFINITY_NEEDLES = ("anon_healthz_keys()", "assert_anon_healthz(", "check_anon_healthz_keys(", "ANON_HEALTHZ_KEYS")
AFFINITY_KEY = "hzkeys"
AFFINITY = {}  # stem -> AFFINITY_KEY, filled by compute_plan() from file content


def is_affinity(stem, content):
    return not stem.startswith("suite_") and any(n in content for n in AFFINITY_NEEDLES)


def check_affinity(suite_files):
    """Die unless every AFFINITY stem sits in exactly one suite file."""
    homes = {}
    for path, content in suite_files.items():
        for stem in re.findall(r'#\[path\s*=\s*"([^"/]+)\.rs"\]', content):
            if stem in AFFINITY:
                homes.setdefault(path, []).append(stem)
    if len(homes) > 1:
        detail = "; ".join(f"{os.path.basename(p)}: {', '.join(sorted(v))}" for p, v in sorted(homes.items()))
        die("files reading the anonymous healthz key set must share ONE suite "
            f"(tests/hzkeys_ac02_handler_body_equals_const.rs), but are split: {detail}")


def area_key(stem):
    if stem in AFFINITY:
        return AFFINITY_KEY
    m = AC_PREFIX_RE.match(stem)
    if m:
        return "ac"
    m = PREFIX_RE.match(stem)
    if m:
        return m.group(1)
    return stem


def bucketize(groups, cap):
    """groups: sorted [(area_key, [stem, ...]), ...]. Returns a list of
    buckets (each a flat, alphabetically-ordered list of stems), each with
    at most `cap` members except when a single group alone exceeds it (kept
    together rather than split -- splitting one area across suites would
    make its own name ambiguous)."""
    buckets = []
    cur = []
    cur_size = 0
    for _key, members in groups:
        if cur and cur_size + len(members) > cap:
            buckets.append(cur)
            cur = []
            cur_size = 0
        cur.extend(members)
        cur_size += len(members)
    if cur:
        buckets.append(cur)
    return buckets


MOD_COMMON_RE = re.compile(r'^mod\s+common\s*;\s*$')
MOD_CI_SANDBOX_RE = re.compile(r'^mod\s+ci_sandbox_support\s*;\s*$')
USE_COMMON_RE = re.compile(r'^use\s+crate::common\s*;\s*$')
USE_CI_SANDBOX_RE = re.compile(r'^use\s+crate::ci_sandbox_support\s*;\s*$')
# A member that names `crate::common::x` / `crate::ci_sandbox_support::x` by full path (no `use` line)
# needs the suite to declare the module too, or it fails E0433 once it sits in a suite with no other user.
FQ_COMMON_RE = re.compile(r'\bcrate::common::')
FQ_CI_SANDBOX_RE = re.compile(r'\bcrate::ci_sandbox_support::')
PATH_MOD_RE = re.compile(r'^#\[path\s*=\s*"(support/(\w+)\.rs)"\]\s*$')
MOD_NAME_RE = re.compile(r'^mod\s+(\w+)\s*;\s*$')
USE_ANY_RE = re.compile(r'^use\s+crate::(\w+)\s*;\s*$')


def existing_support_decls():
    """name -> declaration block (tuple of lines, attrs included), scraped
    from the currently-on-disk generated suite files. A helper's full
    declaration (e.g. the `#[allow(dead_code)]` some carry) needs to survive
    a regen even after every member file that uses it has already been
    migrated to `use crate::<name>;` and no longer carries the raw
    `#[path = ...] mod <name>;` block this run could otherwise learn it
    from."""
    cache = {}
    for suite_path in sorted(glob.glob("tests/suite_*.rs")):
        if not GENERATED_SUITE_RE.match(os.path.basename(suite_path)):
            continue
        with open(suite_path, encoding="utf-8") as fh:
            lines = fh.read().split("\n")
        i = 0
        while i < len(lines):
            m = PATH_MOD_RE.match(lines[i])
            if m:
                name = m.group(2)
                block = [lines[i]]
                j = i + 1
                while j < len(lines) and lines[j].startswith("#["):
                    block.append(lines[j])
                    j += 1
                if j < len(lines) and MOD_NAME_RE.match(lines[j]) and MOD_NAME_RE.match(lines[j]).group(1) == name:
                    block.append(lines[j])
                    cache.setdefault(name, tuple(block))
                    i = j + 1
                    continue
            i += 1
    return cache


SUPPORT_DECL_CACHE = existing_support_decls()


def migrate_content(content):
    """Rewrite a member file's bare `mod common;` / `mod ci_sandbox_support;`
    line to `use crate::common;` / `use crate::ci_sandbox_support;` (once
    each is declared at the including suite's crate root, path resolution
    for `common::`/`ci_sandbox_support::` inside a nested module already
    works via Rust 2018+ uniform paths -- the `use crate::...;` just makes
    that explicit and keeps `common::Foo`/`ci_sandbox_support::Foo`
    references in the file unchanged). Any other member-owned
    `#[path = "support/<name>.rs"] mod <name>;` declaration (optionally
    preceded by attribute lines like `#[allow(dead_code)]`) gets the same
    treatment, rewritten to `use crate::<name>;`, so the same physical file
    is never loaded as a module more than once within one suite binary
    (clippy::duplicate_mod).
    Returns (new_content, needs_common, needs_ci_sandbox_support,
    support_helpers) where support_helpers is a list of (name, decl_lines)
    tuples, decl_lines a tuple of the original declaration's lines (attrs
    included). Idempotent: a file already migrated (has `use crate::<x>;`
    and no bare `mod <x>;` / `#[path=...] mod <x>;`) is detected the same
    way and left byte-for-byte unchanged; support_helpers still reports the
    helper (sourced from SUPPORT_DECL_CACHE, or a minimal synthesized
    declaration if no suite file has ever carried it before) so the suite
    that includes this file still hoists the declaration it depends on.
    """
    lines = content.split("\n")
    needs_common = False
    needs_ci_sandbox = False
    support_helpers = []
    out = []
    i = 0
    n = len(lines)
    while i < n:
        line = lines[i]
        if MOD_COMMON_RE.match(line):
            needs_common = True
            out.append("use crate::common;")
            i += 1
            continue
        if MOD_CI_SANDBOX_RE.match(line):
            needs_ci_sandbox = True
            out.append("use crate::ci_sandbox_support;")
            i += 1
            continue
        if USE_COMMON_RE.match(line) or FQ_COMMON_RE.search(line):
            needs_common = True
        if USE_CI_SANDBOX_RE.match(line) or FQ_CI_SANDBOX_RE.search(line):
            needs_ci_sandbox = True
        m = PATH_MOD_RE.match(line)
        if m:
            path, name = m.group(1), m.group(2)
            block = [line]
            j = i + 1
            while j < n and lines[j].startswith("#["):
                block.append(lines[j])
                j += 1
            if j < n and MOD_NAME_RE.match(lines[j]) and MOD_NAME_RE.match(lines[j]).group(1) == name:
                block.append(lines[j])
                support_helpers.append((name, tuple(block)))
                out.append(f"use crate::{name};")
                i = j + 1
                continue
        mu = USE_ANY_RE.match(line)
        if mu and mu.group(1) not in ("common", "ci_sandbox_support"):
            name = mu.group(1)
            cand_path = f"support/{name}.rs"
            if os.path.isfile(os.path.join("tests", cand_path)):
                decl = SUPPORT_DECL_CACHE.get(
                    name, (f'#[path = "{cand_path}"]', f"mod {name};")
                )
                support_helpers.append((name, decl))
        out.append(line)
        i += 1
    return "\n".join(out), needs_common, needs_ci_sandbox, support_helpers


def suite_content(members_meta):
    """members_meta: list of (stem, needs_common, needs_ci_sandbox,
    support_helpers), already sorted alphabetically by stem."""
    parts = [SUITE_HEADER_TMPL]
    if any(m[1] for m in members_meta):
        parts.append("mod common;\n")
    if any(m[2] for m in members_meta):
        parts.append("mod ci_sandbox_support;\n")
    support_seen = {}
    for _stem, _c, _s, helpers in members_meta:
        for name, decl in helpers:
            support_seen.setdefault(name, decl)
    for name in sorted(support_seen):
        parts.append("\n".join(support_seen[name]) + "\n")
    parts.append("\n")
    for stem, _c, _s, _h in members_meta:
        parts.append(f'#[path = "{stem}.rs"]\nmod {stem.replace("-", "_")};\n')
    return "".join(parts)


# Env suites hold a small minority of files (every test that touches
# EnvGuard/AdvisoryModeGuard, a few dozen at most) -- unlike MAX_PER_SUITE,
# nothing here has ever needed per-PRD retuning, so one generous cap keeps
# each partition's env-guarded files in a single suite_env_<part>_01.rs
# until that stops being true.
ENV_MAX_PER_SUITE = {"core": 500, "sandbox": 500}
# Contract suites are a handful of files; one generous cap, never retuned.
CONTRACT_MAX_PER_SUITE = {"core": 500, "sandbox": 500}
# Cargo-spawning suites: a couple of files, one generous cap.
CARGO_MAX_PER_SUITE = {"core": 500, "sandbox": 500}


def plan_buckets(stems, exclusive, cap):
    """The buckets (exclusive singletons first) one partition packs into at
    per-suite file budget `cap`."""
    grouped = {}
    for stem in sorted(stems):
        grouped.setdefault(area_key(stem), []).append(stem)
    groups = [(k, sorted(grouped[k])) for k in sorted(grouped)]
    return distribute_exclusive(bucketize(groups, cap), exclusive)


def build_suites(name_prefix, cap_by_partition, stems_by_partition, exclusive_by_partition, migrations, suite_files):
    """Buckets `stems_by_partition[part]` (+ `exclusive_by_partition[part]`
    as singletons) into `<name_prefix>_<part>_NN` suites, same area-key
    bucketing every suite kind here uses, writing each suite's content into
    `suite_files` and returning `{part: [suite_name, ...]}`."""
    names_by_partition = {}
    for part in ("core", "sandbox"):
        buckets = plan_buckets(stems_by_partition[part], exclusive_by_partition[part], cap_by_partition[part])
        names = []
        for i, bucket in enumerate(buckets, 1):
            name = f"{name_prefix}_{part}_{i:02d}"
            names.append(name)
            meta = []
            for stem in sorted(bucket):
                path = f"tests/{stem}.rs"
                _content, needs_common, needs_ci_sandbox, support_helpers = migrations[path]
                meta.append((stem, needs_common, needs_ci_sandbox, support_helpers))
            suite_files[f"tests/{name}.rs"] = suite_content(meta)
        names_by_partition[part] = names
    return names_by_partition


def compute_plan():
    all_files = sorted(
        f for f in glob.glob("tests/*.rs")
        if not GENERATED_SUITE_RE.match(os.path.basename(f))
    )
    if not all_files:
        die("no tests/*.rs files found")

    migrations = {}  # path -> (new_content, needs_common, needs_ci_sandbox, support_helpers)
    by_partition = {"core": [], "sandbox": []}
    exclusive_by_partition = {"core": [], "sandbox": []}
    env_by_partition = {"core": [], "sandbox": []}
    env_exclusive_by_partition = {"core": [], "sandbox": []}
    contract_by_partition = {"core": [], "sandbox": []}
    contract_exclusive_by_partition = {"core": [], "sandbox": []}
    cargo_by_partition = {"core": [], "sandbox": []}
    cargo_exclusive_by_partition = {"core": [], "sandbox": []}
    classify_all(all_files)
    for f in all_files:
        with open(f, encoding="utf-8") as fh:
            content = fh.read()
        part = classify(f)
        new_content, needs_common, needs_ci_sandbox, support_helpers = migrate_content(content)
        migrations[f] = (new_content, needs_common, needs_ci_sandbox, support_helpers)
        stem = os.path.basename(f)[:-3]
        if is_affinity(stem, content):
            AFFINITY[stem] = AFFINITY_KEY
        env = is_env_guarded(content)
        solo = solo_weight(content)
        if solo is not None:
            SOLO_WEIGHTS[stem] = solo
        exclusive = is_exclusive_global(content) or solo is not None
        if is_contract(content):
            (contract_exclusive_by_partition if exclusive else contract_by_partition)[part].append(stem)
        elif spawns_cargo(content) and not exclusive:
            # an exclusive / solo-suite spawner is already a singleton binary: it shares a process with nothing
            cargo_by_partition[part].append(stem)
        elif env and exclusive:
            env_exclusive_by_partition[part].append(stem)
        elif env:
            env_by_partition[part].append(stem)
        elif exclusive:
            exclusive_by_partition[part].append(stem)
        else:
            by_partition[part].append(stem)

    caps = dict(MAX_PER_SUITE)
    # The sandbox shard floor counts env sandbox suites too (see the
    # SANDBOX_SUITES check at the bottom), so the plain sandbox partition may
    # shrink to SHARDS minus however many env sandbox suites exist.
    env_names = build_suites("suite_env", ENV_MAX_PER_SUITE, env_by_partition, env_exclusive_by_partition, migrations, {})
    contract_names = build_suites("suite_contracts", CONTRACT_MAX_PER_SUITE, contract_by_partition, contract_exclusive_by_partition, migrations, {})
    cargo_names = build_suites("suite_cargo", CARGO_MAX_PER_SUITE, cargo_by_partition, cargo_exclusive_by_partition, migrations, {})
    min_buckets = {"core": 1, "sandbox": max(1, sandbox_shard_count() - len(env_names["sandbox"]) - len(contract_names["sandbox"]) - len(cargo_names["sandbox"]))}
    def n_buckets(part, cap):
        return len(plan_buckets(by_partition[part], exclusive_by_partition[part], cap))

    while n_buckets("core", caps["core"]) + n_buckets("sandbox", caps["sandbox"]) > MAX_SUITES:
        # sandbox first: its bucket count is pinned near the shard count, so
        # a 3rd sandbox bucket is the usual overflow; core only if sandbox
        # cannot shrink. A partition is grown to the smallest cap that
        # REMOVES a bucket; one that cannot (exclusive singletons, shard
        # floor) is skipped rather than grown forever.
        grew = False
        for part in ("sandbox", "core"):
            cur = n_buckets(part, caps[part])
            if cur <= min_buckets[part]:
                continue
            for cand in range(caps[part] + 1, len(by_partition[part]) + 1):
                if n_buckets(part, cand) < cur:
                    caps[part] = cand
                    grew = True
                    break
            if grew:
                break
        if not grew:
            die(f"cannot pack the core+sandbox suites into MAX_SUITES={MAX_SUITES}: "
                f"core needs {n_buckets('core', caps['core'])} "
                f"({len(exclusive_by_partition['core'])} exclusive singletons), "
                f"sandbox needs {n_buckets('sandbox', caps['sandbox'])} "
                f"({len(exclusive_by_partition['sandbox'])} exclusive singletons, floor {min_buckets['sandbox']}) "
                "and no per-suite budget removes another bucket -- raise MAX_SUITES in this file "
                "and tests/suite_ac1_ten_binaries_and_names_preserved.rs deliberately, or merge/remove "
                "exclusive-global test files")

    suite_files = {}  # path -> content
    suite_names_by_partition = build_suites(
        "suite", caps, by_partition, exclusive_by_partition, migrations, suite_files
    )
    env_suite_names_by_partition = build_suites(
        "suite_env", ENV_MAX_PER_SUITE, env_by_partition, env_exclusive_by_partition, migrations, suite_files
    )

    contract_suite_names_by_partition = build_suites(
        "suite_contracts", CONTRACT_MAX_PER_SUITE, contract_by_partition, contract_exclusive_by_partition, migrations, suite_files
    )

    cargo_suite_names_by_partition = build_suites(
        "suite_cargo", CARGO_MAX_PER_SUITE, cargo_by_partition, cargo_exclusive_by_partition, migrations, suite_files
    )

    check_affinity(suite_files)

    all_suite_names = sorted(
        suite_names_by_partition["core"] + suite_names_by_partition["sandbox"]
        + contract_suite_names_by_partition["core"] + contract_suite_names_by_partition["sandbox"]
        + env_suite_names_by_partition["core"] + env_suite_names_by_partition["sandbox"]
        + cargo_suite_names_by_partition["core"] + cargo_suite_names_by_partition["sandbox"]
    )

    with open("Cargo.toml", encoding="utf-8") as fh:
        cargo_toml = fh.read()
    new_cargo_toml = render_cargo_toml(cargo_toml, all_suite_names)

    return migrations, suite_files, new_cargo_toml, all_files


def render_cargo_toml(cargo_toml, suite_names):
    lines = cargo_toml.split("\n")

    # Strip any previously generated block first (idempotent regen).
    out = []
    in_block = False
    for line in lines:
        if line.strip() == GEN_MARK_BEGIN:
            in_block = True
            continue
        if line.strip() == GEN_MARK_END:
            in_block = False
            continue
        if in_block:
            continue
        out.append(line)
    lines = out

    # Ensure `autotests = false` sits right under [package]'s first line
    # (idempotent: skip if already present anywhere).
    if not any(re.match(r'^\s*autotests\s*=\s*false\b', l) for l in lines):
        for i, line in enumerate(lines):
            if line.strip() == "[package]":
                lines.insert(i + 1, "autotests = false  # gen-test-suites.sh: explicit [[test]] suites below")
                break
        else:
            die("Cargo.toml has no [package] section")

    # Drop a trailing run of blank lines so the appended block doesn't
    # accumulate extra blank lines across regenerations.
    while lines and lines[-1] == "":
        lines.pop()

    block = [GEN_MARK_BEGIN]
    for name in suite_names:
        block.append("")
        block.append("[[test]]")
        block.append(f'name = "{name}"')
        block.append(f'path = "tests/{name}.rs"')
    block.append("")
    block.append(GEN_MARK_END)

    lines.append("")
    lines.extend(block)
    lines.append("")
    return "\n".join(lines)


def suite_member_count(content):
    """Test files (not support modules) a suite file includes."""
    return sum(1 for m in re.findall(r'#\[path\s*=\s*"([^"]+\.rs)"\]', content) if "/" not in m)


def suite_partition(name):
    return "sandbox" if "_sandbox_" in name else "core"


def read_weights():
    try:
        with open(WEIGHTS_REL, encoding="utf-8") as fh:
            data = json.load(fh)
    except (OSError, ValueError):
        return None
    return data if isinstance(data, dict) else None


def solo_suite_weights(suite_files):
    """{suite name: declared seconds} for every generated suite whose ONE member
    file carries a `//! solo-suite: <seconds>` line (SOLO_WEIGHTS)."""
    out = {}
    for path, content in suite_files.items():
        members = [m for m in re.findall(r'#\[path\s*=\s*"([^"/]+)\.rs"\]', content)]
        if len(members) == 1 and members[0] in SOLO_WEIGHTS:
            out[os.path.basename(path)[:-3]] = SOLO_WEIGHTS[members[0]]
    return out


def plan_weights(suite_files):
    """The weights file this plan implies: keys = generated suites (+ `lib`
    when present). Measured values stay when the suite exists on disk with the
    same member count; a re-cut suite (members moved between buckets) is the sum of
    its files' seconds-per-file in the buckets they came from, so a bucket that
    takes over the heavy tail of an old one inherits that weight; a suite none of
    whose files has a recorded origin keeps its own seconds-per-file x the new
    count, else file count x its partition's measured seconds-per-file (>= 1 s)."""
    old = read_weights() or {}
    old_counts = {}
    old_members = {}
    for path in glob.glob("tests/suite_*.rs"):
        base = os.path.basename(path)
        if GENERATED_SUITE_RE.match(base):
            with open(path, encoding="utf-8") as fh:
                text = fh.read()
            old_counts[base[:-3]] = suite_member_count(text)
            old_members[base[:-3]] = [m for m in re.findall(r'#\[path\s*=\s*"([^"]+\.rs)"\]', text) if "/" not in m]
    secs = {"core": 0.0, "sandbox": 0.0}
    files = {"core": 0, "sandbox": 0}
    for name, w in old.items():
        n = old_counts.get(name, 0)
        # only NORMAL buckets set the seconds-per-file rate: singleton suites
        # (exclusive-global / solo files) and the env/contract suites say nothing
        # about what a file of a normal bucket costs
        if n > 1 and re.match(r"suite_(core|sandbox)_\d+$", name) and isinstance(w, (int, float)):
            secs[suite_partition(name)] += w
            files[suite_partition(name)] += n
    rate = {p: (secs[p] / files[p] if files[p] else 1.0) for p in secs}
    # seconds a file cost in the NORMAL bucket it ran in before this plan
    file_secs = {}
    for name, w in old.items():
        n = old_counts.get(name, 0)
        if n > 1 and re.match(r"suite_(core|sandbox)_\d+$", name) and isinstance(w, (int, float)):
            for m in old_members[name]:
                file_secs[m] = w / n
    out = {}
    if "lib" in old:
        out["lib"] = old["lib"]
    solo = solo_suite_weights(suite_files)
    for path, content in suite_files.items():
        name = os.path.basename(path)[:-3]
        n = suite_member_count(content)
        prev = old.get(name)
        if name in solo:
            out[name] = solo[name]  # declared by the file's own `//! solo-suite:` line
        elif isinstance(prev, (int, float)) and old_counts.get(name) == n:
            out[name] = prev
        elif n > 1 and any(m in file_secs for m in re.findall(r'#\[path\s*=\s*"([^"]+\.rs)"\]', content) if "/" not in m):
            part = suite_partition(name)
            mem = [m for m in re.findall(r'#\[path\s*=\s*"([^"]+\.rs)"\]', content) if "/" not in m]
            out[name] = max(1, round(sum(file_secs.get(m, rate[part]) for m in mem)))
        elif isinstance(prev, (int, float)) and old_counts.get(name):
            out[name] = max(1, round(prev * n / old_counts[name]))
        else:
            out[name] = max(1, round(n * rate[suite_partition(name)]))
    return dict(sorted(out.items()))


MEASURED_REL = "scripts/ci-suite-measured.json"
# The measured file is a flat suite -> seconds map. A suite seeded from its recorded
# WEIGHT (no real measurement exists anywhere yet) is listed by name under this
# reserved key; ci-test-partition.sh's stale-ratio check skips the listed names and
# ci-refresh-weights.sh drops the key when it writes the real measurements.
PROVISIONAL_KEY = "_provisional"
# Test-only override of where the BRANCH's measured copy is read from (default: HEAD).
BRANCH_MEASURED_ENV = "GEN_SUITES_BRANCH_MEASURED"


def branch_measured():
    """(suite -> seconds, provisional names) of the branch's own committed
    scripts/ci-suite-measured.json: `git show HEAD:<file>`. wm-build's land.generated entry
    runs `git checkout origin/main -- <measured> <weights>` before this script, and that
    changes the worktree and index only, so HEAD still holds the branch's real
    measurements -- which the main copy lacks for any suite only the branch generates.
    ({}, set()) when there is no such copy (not a repo, no HEAD, no file, unreadable)."""
    override = os.environ.get(BRANCH_MEASURED_ENV)
    try:
        if override:
            with open(override, encoding="utf-8") as fh:
                text = fh.read()
        else:
            top = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True)
            if top.returncode != 0 or os.path.realpath(top.stdout.strip()) != os.path.realpath(REPO_ROOT):
                return {}, set()  # an enclosing repo's HEAD is not this tree's branch
            shown = subprocess.run(["git", "show", f"HEAD:{MEASURED_REL}"], capture_output=True, text=True)
            if shown.returncode != 0:
                return {}, set()
            text = shown.stdout
        data = json.loads(text)
    except (OSError, ValueError):
        return {}, set()
    if not isinstance(data, dict):
        return {}, set()
    prov = data.get(PROVISIONAL_KEY)
    prov = {n for n in prov if isinstance(n, str)} if isinstance(prov, list) else set()
    vals = {k: v for k, v in data.items() if isinstance(v, (int, float)) and not isinstance(v, bool)}
    return vals, prov


def plan_measured(suite_files, new_weights):
    """scripts/ci-suite-measured.json as this plan implies it, or None when the
    file is absent (ci-refresh-weights.sh creates it). The measured seconds of a
    suite belong to the member set CI ran: a resized suite's entry is scaled by
    the factor plan_weights applied to its recorded weight (new / old recorded
    seconds), so their ratio -- what the stale check reads -- survives the
    re-pack whatever per-file costs moved; a suite this plan drops has no entry. A
    suite this plan CREATES is seeded from the suites its files came from: each member
    file carries its source suite's measured / recorded ratio (weighted by the file's
    share of that source's recorded seconds), and the new suite's entry is its new
    recorded weight times that ratio -- for a split from ONE source, exactly
    measured x w_new / w_old, the scaling a resized suite gets above. Without the seed
    a created suite had no entry until the next refresh, and a `check` that requires
    an entry for every suite would fail every shard before its tests print the lines
    the refresh reads (a deadlock after any future split).
    Otherwise ci-test-partition.sh check compares the OLD set's seconds with the
    NEW set's weight (ryzen7 run 404, 10-18: core_08 re-packed to recorded 223 s
    against measured 398 s, and the land precheck refused).
    Finally every suite of the GENERATED set still without an entry is seeded, so no
    generated suite ever lacks one (run 1034, 10-21: the land's `git checkout origin/main --
    <measured> <weights>` handed this script a main copy without suite_core_12, which the
    branch generates, and `check` refused "unmeasured suite"; this loop only ever walked the
    keys of the file it read). Source A: the branch's committed value (`branch_measured`).
    Source B, when the branch has none either: the suite's computed weight, listed under
    PROVISIONAL_KEY. One stdout line per seed."""
    try:
        with open(MEASURED_REL, encoding="utf-8") as fh:
            old = json.load(fh)
    except (OSError, ValueError):
        return None
    if not isinstance(old, dict):
        return None
    prov_old = old.pop(PROVISIONAL_KEY, [])  # not a suite: carried forward below
    prov_old = {n for n in prov_old if isinstance(n, str)} if isinstance(prov_old, list) else set()
    old_counts = {}
    old_members = {}
    for path in glob.glob("tests/suite_*.rs"):
        base = os.path.basename(path)
        if GENERATED_SUITE_RE.match(base):
            with open(path, encoding="utf-8") as fh:
                text = fh.read()
            old_counts[base[:-3]] = suite_member_count(text)
            old_members[base[:-3]] = [m for m in re.findall(r'#\[path\s*=\s*"([^"]+\.rs)"\]', text) if "/" not in m]
    new_counts = {os.path.basename(p)[:-3]: suite_member_count(c) for p, c in suite_files.items()}
    old_weights = read_weights() or {}
    out = {}
    for name, secs in old.items():
        if not GENERATED_SUITE_RE.match(name + ".rs"):
            out[name] = secs  # not a generated suite (lib, bin:*): the plan says nothing about it
            continue
        n, o = new_counts.get(name), old_counts.get(name)
        if n is None or not o or not isinstance(secs, (int, float)):
            continue
        if n == o:
            out[name] = secs
            continue
        w_old, w_new = old_weights.get(name), new_weights.get(name)
        if isinstance(w_old, (int, float)) and w_old > 0 and isinstance(w_new, (int, float)):
            out[name] = round(secs * w_new / w_old, 2)
    # created suites: seeded from the old suites their member files came from
    ratio_of_file = {}  # member file -> (share of its source's recorded seconds, source measured / recorded)
    for name, secs in old.items():
        w_old = old_weights.get(name)
        if name in old_members and old_counts.get(name) and isinstance(secs, (int, float)) \
                and isinstance(w_old, (int, float)) and w_old > 0:
            for m in old_members[name]:
                ratio_of_file[m] = (w_old / old_counts[name], secs / w_old)
    for path, content in suite_files.items():
        name = os.path.basename(path)[:-3]
        w_new = new_weights.get(name)
        if name in old_counts or name in out or not isinstance(w_new, (int, float)):
            continue
        members = [m for m in re.findall(r'#\[path\s*=\s*"([^"]+\.rs)"\]', content) if "/" not in m]
        den = sum(ratio_of_file[m][0] for m in members if m in ratio_of_file)
        if den > 0:
            num = sum(ratio_of_file[m][0] * ratio_of_file[m][1] for m in members if m in ratio_of_file)
            out[name] = round(w_new * num / den, 2)
    # every generated suite still without an entry: the branch's committed value, else its weight (provisional)
    prov = {n for n in prov_old if n in out}
    head, head_prov = branch_measured()
    for path in sorted(suite_files):
        name = os.path.basename(path)[:-3]
        if name in out:
            continue
        w_new = new_weights.get(name)
        if name in head:
            out[name], src = head[name], "head"
            if name in head_prov:
                prov.add(name)
        elif isinstance(w_new, (int, float)):
            out[name], src = w_new, "weight-provisional"
            prov.add(name)
        else:
            continue
        print(f"gen-test-suites: seeded measured {name} = {out[name]} (src={src})")
    if prov:
        out[PROVISIONAL_KEY] = sorted(prov)
    return dict(sorted(out.items()))


def write_plan(migrations, suite_files, new_cargo_toml, all_files):
    changed = []
    new_weights = plan_weights(suite_files)  # reads the OLD suite files, so before they are rewritten
    new_measured = plan_measured(suite_files, new_weights)  # same: reads the OLD suite files and weights
    for path, (new_content, _c, _s, _h) in migrations.items():
        with open(path, encoding="utf-8") as fh:
            old_content = fh.read()
        if old_content != new_content:
            with open(path, "w", encoding="utf-8") as fh:
                fh.write(new_content)
            changed.append(path)

    existing_suites = {p for p in glob.glob("tests/suite_*.rs") if GENERATED_SUITE_RE.match(os.path.basename(p))}
    desired_suites = set(suite_files)
    for stray in sorted(existing_suites - desired_suites):
        os.remove(stray)
        changed.append(f"removed {stray}")
    for path, content in sorted(suite_files.items()):
        old_content = None
        if os.path.exists(path):
            with open(path, encoding="utf-8") as fh:
                old_content = fh.read()
        if old_content != content:
            with open(path, "w", encoding="utf-8") as fh:
                fh.write(content)
            changed.append(path)

    with open("Cargo.toml", encoding="utf-8") as fh:
        old_cargo = fh.read()
    if old_cargo != new_cargo_toml:
        with open("Cargo.toml", "w", encoding="utf-8") as fh:
            fh.write(new_cargo_toml)
        changed.append("Cargo.toml")

    if read_weights() != new_weights:
        with open(WEIGHTS_REL, "w", encoding="utf-8") as fh:
            fh.write(json.dumps(new_weights, indent=2) + "\n")
        changed.append(WEIGHTS_REL)

    if new_measured is not None:
        with open(MEASURED_REL, encoding="utf-8") as fh:
            old_measured = json.load(fh)
        if old_measured != new_measured:
            with open(MEASURED_REL, "w", encoding="utf-8") as fh:
                fh.write(json.dumps(new_measured, indent=2) + "\n")
            changed.append(MEASURED_REL)

    if changed:
        print("gen-test-suites.sh: wrote/updated:")
        for c in changed:
            print(f"  {c}")
    else:
        print("gen-test-suites.sh: already up to date")
    print(f"gen-test-suites.sh: {len(suite_files)} suites over {len(all_files)} test files")


def check_plan(migrations, suite_files, new_cargo_toml, all_files):
    problems = []

    # Coverage: every member file must be referenced by exactly one suite
    # file ALREADY ON DISK (this is what actually catches a brand-new,
    # never-generated tests/<name>.rs -- AC2).
    referenced = set()
    for suite_path in glob.glob("tests/suite_*.rs"):
        if not GENERATED_SUITE_RE.match(os.path.basename(suite_path)):
            continue
        with open(suite_path, encoding="utf-8") as fh:
            content = fh.read()
        for m in re.finditer(r'#\[path\s*=\s*"([^"]+\.rs)"\]', content):
            referenced.add(m.group(1))
    for f in all_files:
        base = os.path.basename(f)
        if base not in referenced:
            problems.append(f"{f} is not included in any suite (run gen-test-suites.sh)")

    # Contract axis: a marked file must live in exactly one contract suite,
    # and a contract suite may hold only marked files.
    marked = {}
    for path in all_files:
        with open(path, encoding="utf-8") as fh:
            marked[os.path.basename(path)] = is_contract(fh.read())
    for suite_path in sorted(glob.glob("tests/suite_*.rs")):
        sbase = os.path.basename(suite_path)
        if not GENERATED_SUITE_RE.match(sbase):
            continue
        is_contract_suite = sbase.startswith("suite_contracts_")
        with open(suite_path, encoding="utf-8") as fh:
            members = re.findall(r'#\[path\s*=\s*"([^"]+\.rs)"\]', fh.read())
        for m in members:
            if "/" in m:
                continue  # shared support module, not a test file
            if is_contract_suite and not marked.get(m, False):
                problems.append(f"tests/{m} is in {sbase} but has no `//! contract:` line in its first 10 lines")
            if not is_contract_suite and marked.get(m, False):
                problems.append(f"tests/{m} carries `//! contract:` but is included by {sbase}, not a suite_contracts_<core|sandbox>_NN.rs (run gen-test-suites.sh)")

    # Packing axis: no generated binary holds a file that spawns cargo beside a
    # file that execs the built binary (the uplift race).
    for suite_path in sorted(glob.glob("tests/suite_*.rs")):
        sbase = os.path.basename(suite_path)
        if not GENERATED_SUITE_RE.match(sbase):
            continue
        with open(suite_path, encoding="utf-8") as fh:
            members = [m for m in re.findall(r'#\[path\s*=\s*"([^"]+\.rs)"\]', fh.read()) if "/" not in m]
        spawners, execers = [], []
        for m in members:
            try:
                with open(os.path.join("tests", m), encoding="utf-8") as fh:
                    text = fh.read()
            except OSError:
                continue
            if spawns_cargo(text):
                spawners.append(m)
            if execs_bin(text):
                execers.append(m)
        for a in spawners:
            for b in execers:
                if a != b:
                    problems.append(f"tests/{a} (spawns cargo) and tests/{b} (execs CARGO_BIN_EXE_) share {sbase[:-3]}")

    for path, (new_content, _c, _s, _h) in migrations.items():
        if not os.path.exists(path):
            problems.append(f"{path} missing")
            continue
        with open(path, encoding="utf-8") as fh:
            old_content = fh.read()
        if old_content != new_content:
            problems.append(f"{path} has drifted (mod common;/mod ci_sandbox_support; not migrated, or hand-edited)")

    existing_suites = {p for p in glob.glob("tests/suite_*.rs") if GENERATED_SUITE_RE.match(os.path.basename(p))}
    desired_suites = set(suite_files)
    for stray in sorted(existing_suites - desired_suites):
        problems.append(f"{stray} exists but gen-test-suites.sh would not generate it (stale)")
    for path, content in sorted(suite_files.items()):
        if not os.path.exists(path):
            problems.append(f"{path} missing (run gen-test-suites.sh)")
            continue
        with open(path, encoding="utf-8") as fh:
            old_content = fh.read()
        if old_content != content:
            problems.append(f"{path} has drifted from the generator's output")

    with open("Cargo.toml", encoding="utf-8") as fh:
        old_cargo = fh.read()
    if old_cargo != new_cargo_toml:
        problems.append("Cargo.toml's generated [[test]] block (or autotests = false) has drifted")

    # The weights file's keys must be exactly the generated suite set (+ lib).
    have = read_weights()
    want = {os.path.basename(p)[:-3] for p in suite_files}
    if have is None:
        problems.append(f"{WEIGHTS_REL} is missing or not a JSON object (run gen-test-suites.sh)")
    else:
        for name in sorted(want - set(have)):
            problems.append(f"{WEIGHTS_REL} has no weight for suite {name} (run gen-test-suites.sh)")
        for name, secs in sorted(solo_suite_weights(suite_files).items()):
            if have.get(name) != secs:
                problems.append(f"{WEIGHTS_REL} weighs solo suite {name} at {have.get(name)!r} but its file declares `//! solo-suite: {secs}` (run gen-test-suites.sh)")
        for name in sorted(set(have) - want - {"lib"}):
            problems.append(f"{WEIGHTS_REL} has a stale entry for {name}, which gen-test-suites.sh no longer generates (run gen-test-suites.sh)")

    if problems:
        print("gen-test-suites.sh --check: FAILED", file=sys.stderr)
        for p in problems:
            print(f"  {p}", file=sys.stderr)
        return 1
    print(f"gen-test-suites.sh --check: ok ({len(suite_files)} suites over {len(all_files)} test files)")
    return 0


def sandbox_shard_count():
    """The one SANDBOX_SHARDS definition, read from ci-test-partition.sh."""
    out = subprocess.run([CLASSIFY_SCRIPT, "shards"], capture_output=True, text=True)
    if out.returncode != 0 or not out.stdout.strip().isdigit():
        die(f"`ci-test-partition.sh shards` did not print a count: {out.stdout!r} {out.stderr!r}")
    return int(out.stdout.strip())


def core_shard_count():
    """The one CORE_SHARDS definition, read from ci-test-partition.sh."""
    out = subprocess.run([CLASSIFY_SCRIPT, "shards", "core"], capture_output=True, text=True)
    if out.returncode != 0 or not out.stdout.strip().isdigit():
        die(f"`ci-test-partition.sh shards core` did not print a count: {out.stdout!r} {out.stderr!r}")
    return int(out.stdout.strip())


migrations, suite_files, new_cargo_toml, all_files = compute_plan()
SHARDS = sandbox_shard_count()
SANDBOX_SUITES = [n for n in suite_files if re.match(r"suite(_env|_contracts|_cargo)?_sandbox_\d+\.rs$", os.path.basename(n))]
if len(SANDBOX_SUITES) < SHARDS:
    die(f"{len(SANDBOX_SUITES)} sandbox suites cannot fill {SHARDS} shards "
        "(SANDBOX_SHARDS in scripts/ci-test-partition.sh): lower it")
CORE_SHARDS = core_shard_count()
CORE_SUITES = [n for n in suite_files if re.match(r"suite(_env|_contracts|_cargo)?_core_\d+\.rs$", os.path.basename(n))]
if len(CORE_SUITES) < CORE_SHARDS:
    die(f"{len(CORE_SUITES)} core suites cannot fill {CORE_SHARDS} shards "
        "(CORE_SHARDS in scripts/ci-test-partition.sh): lower it")
if CHECK:
    sys.exit(check_plan(migrations, suite_files, new_cargo_toml, all_files))
else:
    write_plan(migrations, suite_files, new_cargo_toml, all_files)
PY
