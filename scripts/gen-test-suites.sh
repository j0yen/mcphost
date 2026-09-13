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
# suite's crate root, so no other line in any test file changes. This is
# the ONLY edit this script makes to a `tests/*.rs` file's content; the test
# bodies, assertions, and every other `use`/`#[path]` line (e.g. the
# unrelated `#[path = "support/host.rs"] mod host;` some files already use)
# are untouched and keep resolving exactly as before, because `#[path]`
# module resolution is relative to the FILE's own on-disk directory
# (unchanged: files stay where they are), not to how something else
# `#[path]`-includes that file.
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
import os
import re
import subprocess
import sys

MODE = sys.argv[1] if len(sys.argv) > 1 else ""
CHECK = MODE == "--check"

REPO_ROOT = os.getcwd()
CLASSIFY_SCRIPT = os.path.join(REPO_ROOT, "scripts", "ci-test-partition.sh")

# Kept in sync with ci-test-partition.sh's SANDBOX_SHARDS comment: if the
# sandbox cap below ever pushes that partition past 2 suites (or the core
# partition past however many exist today), widen SANDBOX_SHARDS there and
# the matrix in .github/workflows/ci.yml to match. P0 requirement: no suite
# exceeds roughly 60 files, and the total (both partitions) stays <=10.
#
# Per-partition, not one global number (2026-09-12 follow-up): the
# exclusive-global singleton fix above (`distribute_exclusive`) adds one
# whole extra binary per exclusive-subscriber file. Originally all such
# files lived in "core" (5 of them) -- at cap 80, core's non-exclusive
# files bucketed into 3 suites, landing the grand total at 3 + 5 + 2 = 10
# (sandbox's 2-suite split is already load-bearing for
# `.github/workflows/ci.yml`'s SANDBOX_SHARDS=2 matrix -- a 1-suite sandbox
# would starve shard 2 and trip ci-test-partition.sh's own empty-shard
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
# SANDBOX_SHARDS=2 the moment sandbox grows again) so core's cap absorbs it
# instead: raised 80 -> 120, collapsing core's non-exclusive buckets from 3
# to 1, landing the grand total at 1 + 6 + (2 + 1) = 10 -- exactly the AC1
# ceiling, not under it by luck. (Core's exclusive-singleton count moved
# 5 -> 6 in the same follow-up; if core ever needs a second normal bucket
# again, this comment's arithmetic needs a fresh look, same as this one did.)
MAX_PER_SUITE = {"core": 120, "sandbox": 60}

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


def classify(path):
    out = subprocess.run(
        [CLASSIFY_SCRIPT, "classify-file", path],
        capture_output=True, text=True, check=True,
    )
    verdict = out.stdout.strip()
    if verdict not in ("sandbox", "core"):
        die(f"ci-test-partition.sh classify-file {path!r} returned {verdict!r}")
    return verdict


# Matches ONLY this generator's own output file names (tests/suite_core_01.rs,
# tests/suite_sandbox_02.rs, ...) -- deliberately narrower than "starts with
# suite_", because this PRD's own `test_prefix: suite` AC-pairing selftests
# are named tests/suite_ac<N>_*.rs (verified-completed.sh's `prefix:<p>` rule
# needs that exact shape) and must never be mistaken for generated output, or
# a regen would silently delete them as "stray".
GENERATED_SUITE_RE = re.compile(r'^suite_(?:core|sandbox)_[0-9]+\.rs$')

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


def area_key(stem):
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


def migrate_content(content):
    """Rewrite a member file's bare `mod common;` / `mod ci_sandbox_support;`
    line to `use crate::common;` / `use crate::ci_sandbox_support;` (once
    each is declared at the including suite's crate root, path resolution
    for `common::`/`ci_sandbox_support::` inside a nested module already
    works via Rust 2018+ uniform paths -- the `use crate::...;` just makes
    that explicit and keeps `common::Foo`/`ci_sandbox_support::Foo`
    references in the file unchanged).
    Returns (new_content, needs_common, needs_ci_sandbox_support). Idempotent:
    a file already migrated (has `use crate::common;` and no bare `mod
    common;`) is detected as needs_common=True and left byte-for-byte
    unchanged.
    """
    lines = content.split("\n")
    needs_common = False
    needs_ci_sandbox = False
    out = []
    for line in lines:
        if MOD_COMMON_RE.match(line):
            needs_common = True
            out.append("use crate::common;")
            continue
        if MOD_CI_SANDBOX_RE.match(line):
            needs_ci_sandbox = True
            out.append("use crate::ci_sandbox_support;")
            continue
        if USE_COMMON_RE.match(line):
            needs_common = True
        if USE_CI_SANDBOX_RE.match(line):
            needs_ci_sandbox = True
        out.append(line)
    return "\n".join(out), needs_common, needs_ci_sandbox


def suite_content(members_meta):
    """members_meta: list of (stem, needs_common, needs_ci_sandbox), already
    sorted alphabetically by stem."""
    parts = [SUITE_HEADER_TMPL]
    if any(m[1] for m in members_meta):
        parts.append("mod common;\n")
    if any(m[2] for m in members_meta):
        parts.append("mod ci_sandbox_support;\n")
    parts.append("\n")
    for stem, _c, _s in members_meta:
        parts.append(f'#[path = "{stem}.rs"]\nmod {stem};\n')
    return "".join(parts)


def compute_plan():
    all_files = sorted(
        f for f in glob.glob("tests/*.rs")
        if not GENERATED_SUITE_RE.match(os.path.basename(f))
    )
    if not all_files:
        die("no tests/*.rs files found")

    migrations = {}  # path -> (new_content, needs_common, needs_ci_sandbox)
    by_partition = {"core": [], "sandbox": []}
    exclusive_by_partition = {"core": [], "sandbox": []}
    for f in all_files:
        with open(f, encoding="utf-8") as fh:
            content = fh.read()
        part = classify(f)
        new_content, needs_common, needs_ci_sandbox = migrate_content(content)
        migrations[f] = (new_content, needs_common, needs_ci_sandbox)
        stem = os.path.basename(f)[:-3]
        if is_exclusive_global(content):
            exclusive_by_partition[part].append(stem)
        else:
            by_partition[part].append(stem)

    suite_files = {}  # path -> content
    suite_names_by_partition = {}
    for part in ("core", "sandbox"):
        stems = sorted(by_partition[part])
        grouped = {}
        for stem in stems:
            grouped.setdefault(area_key(stem), []).append(stem)
        groups = sorted(grouped.items())
        for k in grouped:
            grouped[k].sort()
        groups = [(k, grouped[k]) for k, _ in groups]
        buckets = bucketize(groups, MAX_PER_SUITE[part])
        buckets = distribute_exclusive(buckets, exclusive_by_partition[part])
        names = []
        for i, bucket in enumerate(buckets, 1):
            name = f"suite_{part}_{i:02d}"
            names.append(name)
            meta = []
            for stem in sorted(bucket):
                path = f"tests/{stem}.rs"
                _content, needs_common, needs_ci_sandbox = migrations[path]
                meta.append((stem, needs_common, needs_ci_sandbox))
            suite_files[f"tests/{name}.rs"] = suite_content(meta)
        suite_names_by_partition[part] = names

    all_suite_names = sorted(suite_names_by_partition["core"] + suite_names_by_partition["sandbox"])

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


def write_plan(migrations, suite_files, new_cargo_toml, all_files):
    changed = []
    for path, (new_content, _c, _s) in migrations.items():
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

    for path, (new_content, _c, _s) in migrations.items():
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

    if problems:
        print("gen-test-suites.sh --check: FAILED", file=sys.stderr)
        for p in problems:
            print(f"  {p}", file=sys.stderr)
        return 1
    print(f"gen-test-suites.sh --check: ok ({len(suite_files)} suites over {len(all_files)} test files)")
    return 0


migrations, suite_files, new_cargo_toml, all_files = compute_plan()
if CHECK:
    sys.exit(check_plan(migrations, suite_files, new_cargo_toml, all_files))
else:
    write_plan(migrations, suite_files, new_cargo_toml, all_files)
PY
