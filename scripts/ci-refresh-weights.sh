#!/usr/bin/env bash
# ci-refresh-weights.sh -- the ONE writer of scripts/ci-suite-measured.json and
# the recorded seconds in scripts/ci-suite-weights.json.
#
# Reads the `suite=<name> measured=<s>s recorded=...` lines that
# scripts/ci-shard-report.sh prints into every shard log (core AND sandbox, at every
# level: a sandbox shard under its warn line used to print none, so a suite recorded
# at 152 s that really ran ~300 s alarmed nothing), takes the MAX measured seconds per
# suite over every input, then:
#   1. writes scripts/ci-suite-measured.json   (suite -> max measured seconds; the reserved
#      `_provisional` key {suite: seed date}, a seed gen-test-suites.sh wrote for a suite made of only new
#      test files, loses every suite this input measures and vanishes when empty; a provisional suite the
#      input does NOT measure exits 1 naming it, nothing written)
#   2. sets each suite's recorded weight to ceil(measured) in
#      scripts/ci-suite-weights.json, except a SOLO suite (its weight is declared
#      by its test file's `//! solo-suite: <seconds>` line: edit that line instead;
#      this script says so) and names that are not weight keys (`bin:*`).
# `ci-test-partition.sh check` fails when a recorded weight is below
# measured / 1.5, which is how a stale weight gets noticed on every PR.
#
# Usage:
#   ci-refresh-weights.sh <log-file|run-id>...   # a run-id is fetched as the run's log ZIP,
#                                                  `gh api repos/j0yen/mcphost/actions/runs/<id>/logs`,
#                                                  every member read with `unzip -p` (needs `unzip`).
#                                                  NEVER `gh run view --log`: it TRUNCATES a long
#                                                  job's log mid-shard (run 39163386677 lost shard 3/5,
#                                                  incl. suite_core_09), dropping suites silently.
#   then: scripts/gen-test-suites.sh && scripts/ci-test-partition.sh check
# Completeness: every suite key of `ci-test-partition.sh list measured-required` (the keys of
# scripts/ci-suite-weights.json except `lib` and `bin:*`: core AND sandbox suites) must have a
# `suite=<name> measured=` line in the input; otherwise this exits 1 naming the missing suites and
# writes NOTHING.
# Membership: a run id's measurement of a suite counts only when the suite's member files at that run's head commit
# (`gh api .../actions/runs/<id>` head_sha, read with `git show`) are a subset of its members now; a run that ran a
# file since moved out of the suite is not counted for that suite (a line names it), and a head commit missing from
# this checkout exits 1, nothing written (fetch-depth 0). ci-refresh-weights-auto.sh passes a WINDOW of green main
# runs, so a suite whose time swings run to run (suite_core_12: 600.56 s on 44757115005, 326.72 s on 45034667949)
# is recorded at its max over them, not at whichever sample came last.
# Env: CI_REFRESH_REPO (default j0yen/mcphost); CI_REFRESH_ROOT (default this checkout; tests use a scratch root);
#   CI_REFRESH_ALLOW_LOCAL=1 to accept a local <log-file> (a coder's local timing must never become the
#   recorded measurement: without it only a CI run id is accepted and a file argument writes NOTHING).
set -uo pipefail
SELF_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${CI_REFRESH_ROOT:-$SELF_ROOT}" || exit 2
[ "$#" -ge 1 ] || { sed -n '2,41p' "$0" >&2; exit 2; }
if [ "${CI_REFRESH_ALLOW_LOCAL:-}" != 1 ]; then
  for arg in "$@"; do
    [ -f "$arg" ] && { echo "ci-refresh-weights: $arg is a local log file; only a CI run id is a measurement (set CI_REFRESH_ALLOW_LOCAL=1 to override); nothing written" >&2; exit 2; }
  done
fi

# One log file per input, listed in $inputs as "<log file>\t<label>\t<head commit>" (no commit for a local file).
logs="$(mktemp -d)"; zip="$(mktemp)"; trap 'rm -rf "$logs" "$zip"' EXIT
inputs="$logs/inputs"; : > "$inputs"
i=0
for arg in "$@"; do
  i=$((i + 1))
  if [ -f "$arg" ]; then
    sed -E $'s/\\x1b\\[[0-9;]*m//g' "$arg" > "$logs/$i"
    printf '%s\tfile %s\t\n' "$logs/$i" "$arg" >> "$inputs"
  else
    command -v unzip >/dev/null 2>&1 \
      || { echo "ci-refresh-weights: unzip is required to read a run id's log ZIP (install unzip); not falling back to the truncating \`gh run view --log\`" >&2; exit 1; }
    gh api "repos/${CI_REFRESH_REPO:-j0yen/mcphost}/actions/runs/$arg/logs" > "$zip" 2>/dev/null \
      && unzip -p "$zip" | sed -E $'s/\\x1b\\[[0-9;]*m//g' > "$logs/$i" \
      || { echo "ci-refresh-weights: cannot fetch the log ZIP of run $arg" >&2; exit 1; }
    sha="$(gh api "repos/${CI_REFRESH_REPO:-j0yen/mcphost}/actions/runs/$arg" 2>/dev/null \
             | python3 -I -c 'import json, sys; print(json.load(sys.stdin).get("head_sha") or "")' 2>/dev/null)"
    [[ "$sha" =~ ^[0-9a-f]{40}$ ]] || { echo "ci-refresh-weights: cannot read the head commit of run $arg" >&2; exit 1; }
    printf '%s\trun %s\t%s\n' "$logs/$i" "$arg" "$sha" >> "$inputs"
  fi
done

# The required key set has ONE emitter (ci-test-partition.sh), read against THIS root's weights file.
required="$(CI_SUITE_WEIGHTS_FILE="$PWD/scripts/ci-suite-weights.json" "$SELF_ROOT/scripts/ci-test-partition.sh" list measured-required)" \
  || { echo "ci-refresh-weights: cannot read the required suite list from ci-test-partition.sh" >&2; exit 2; }

python3 - "$inputs" "$required" <<'PY'
import glob, json, os, re, subprocess, sys

LINE = re.compile(r'\bsuite=(\S+) measured=([0-9.]+)s\b')
SOLO = re.compile(r'^//!\s*solo-suite:\s*([0-9]+)\b')
MEMBER = re.compile(r'#\[path\s*=\s*"([^"]+\.rs)"\]')


def members(text):
    """The test files a generated suite includes (support modules, `support/...`, are not members)."""
    return {m for m in MEMBER.findall(text) if "/" not in m}


# MEMBERSHIP: a run's measurement belongs to the member set CI ran. It counts for a suite only when the suite's
# members at the run's head commit are a SUBSET of its members now (a suite that only gained files since ran less,
# never more); a run that ran files since moved out of the suite would carry their seconds into a suite that no
# longer runs them, so that one measurement is dropped (and named). A local file has no commit and always counts.
at_commit = {}
verdict = {}
def counts(label, sha, name):
    if not sha or not os.path.exists(f"tests/{name}.rs"):
        return True
    if (sha, name) not in verdict:
        verdict[(sha, name)] = judge(label, sha, name)
    return verdict[(sha, name)]


def judge(label, sha, name):
    if sha not in at_commit:
        if subprocess.run(["git", "cat-file", "-e", f"{sha}^{{commit}}"], capture_output=True).returncode != 0:
            sys.exit(f"ci-refresh-weights: {label}: head commit {sha} is not in this checkout (fetch-depth 0 needed); nothing written")
        at_commit[sha] = {}
    if name not in at_commit[sha]:
        r = subprocess.run(["git", "show", f"{sha}:tests/{name}.rs"], capture_output=True, text=True)
        at_commit[sha][name] = members(r.stdout) if r.returncode == 0 else None
    then = at_commit[sha][name]
    if then is None:
        print(f"ci-refresh-weights: {name}: {label} ({sha[:10]}) had no tests/{name}.rs; that measurement is not counted")
        return False
    gone = sorted(then - members(open(f"tests/{name}.rs", encoding="utf-8").read()))
    if gone:
        print(f"ci-refresh-weights: {name}: {label} ({sha[:10]}) ran {len(gone)} file(s) no longer in it ({', '.join(gone[:3])}); that measurement is not counted")
        return False
    return True


measured = {}
for row in open(sys.argv[1], encoding="utf-8"):
    path, label, sha = row.rstrip("\n").split("\t")
    for line in open(path, encoding="utf-8", errors="replace"):
        for name, secs in LINE.findall(line):
            if not name.startswith("bin:") and counts(label, sha, name):
                measured[name] = max(measured.get(name, 0.0), float(secs))
if not measured:
    sys.exit("ci-refresh-weights: no `suite=<name> measured=<s>s` lines in the input")

W, M = "scripts/ci-suite-weights.json", "scripts/ci-suite-measured.json"
weights = json.load(open(W))

old_m = json.load(open(M)) if os.path.exists(M) else {}
# `_provisional` ({suite: {pending, derived_s, since}}, written by gen-test-suites.sh for a suite created from
# only new test files; an older date-valued mark is read the same way, only its keys matter) is bookkeeping, not a suite: a real measurement of the suite clears its mark; a provisional suite
# the input does not measure is an error (it would stay a guess).
provisional = old_m.pop("_provisional", None)
provisional = provisional if isinstance(provisional, dict) else {}
unmeasured_prov = sorted(k for k in provisional if k in weights and k not in measured)
if unmeasured_prov:
    sys.exit("ci-refresh-weights: provisional suite(s) still unmeasured after this refresh: "
             + ", ".join(unmeasured_prov) + " (the input has no `suite=<name> measured=` line for them; nothing written)")

# COMPLETENESS (the one emitter `ci-test-partition.sh list measured-required`, never a hand list):
# every required suite, core and sandbox, must be measured in the input, else a truncated log (or a
# shard that printed nothing) silently drops suites from the measured file.
required = sys.argv[2].split()
missing = sorted(k for k in required if k not in measured)
if missing:
    sys.exit("ci-refresh-weights: input lacks a `suite=<name> measured=` line for suite(s): "
             + ", ".join(missing) + " (truncated log? nothing written)")
# suites whose ONE member file declares a solo-suite weight
solo = set()
for suite in glob.glob("tests/suite_*_[0-9]*.rs"):
    text = open(suite, encoding="utf-8").read()
    members = re.findall(r'#\[path\s*=\s*"([^"/]+)\.rs"\]', text)
    if len(members) == 1:
        head = open(f"tests/{members[0]}.rs", encoding="utf-8").read().split("\n")[:10]
        if any(SOLO.match(l) for l in head):
            solo.add(os.path.basename(suite)[:-3])

new_m = {k: v for k, v in old_m.items() if k in weights}
for name, secs in measured.items():
    if name in weights:
        new_m[name] = round(secs, 2)
    else:
        print(f"ci-refresh-weights: {name} is not a current suite; ignored")
for name, secs in sorted(new_m.items()):
    if name in measured and name in solo:
        print(f"ci-refresh-weights: {name} is a solo suite: edit its file's `//! solo-suite:` line to {int(-(-secs // 1))}")
    elif name in measured and weights.get(name) != int(-(-secs // 1)):
        print(f"ci-refresh-weights: {name} recorded {weights.get(name)} -> {int(-(-secs // 1))}")
        weights[name] = max(1, int(-(-secs // 1)))
# every provisional suite is measured here (else the exit above), so the key is dropped whole
for name in sorted(provisional):
    if name in measured and name in weights:
        print(f"ci-refresh-weights: {name} measured; pending entry replaced by the measurement")
json.dump(dict(sorted(weights.items())), open(W, "w"), indent=2); open(W, "a").write("\n")
json.dump(dict(sorted(new_m.items())), open(M, "w"), indent=2); open(M, "a").write("\n")
print(f"ci-refresh-weights: wrote {M} ({len(new_m)} suites) and {W}")
PY
