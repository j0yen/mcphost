#!/usr/bin/env bash
# ci-refresh-weights.sh -- the ONE writer of scripts/ci-suite-measured.json and
# the recorded seconds in scripts/ci-suite-weights.json.
#
# Reads the `suite=<name> measured=<s>s recorded=...` lines that
# scripts/ci-shard-report.sh prints into every core shard log (and into a sandbox
# shard log at or above its warn line), takes the MAX measured seconds per suite
# over every input, then:
#   1. writes scripts/ci-suite-measured.json   (suite -> max measured seconds)
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
# Completeness: every CORE suite key in scripts/ci-suite-weights.json (not `lib`, `bin:*`, or a
# sandbox suite, which reports only above its warn line) must have a `suite=<name> measured=` line
# in the input; otherwise this exits 1 naming the missing suites and writes NOTHING.
# Env: CI_REFRESH_REPO (default j0yen/mcphost); CI_REFRESH_ROOT (default this checkout; tests use a scratch root).
set -uo pipefail
cd "${CI_REFRESH_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}" || exit 2
[ "$#" -ge 1 ] || { sed -n '2,29p' "$0" >&2; exit 2; }

tmp="$(mktemp)"; zip="$(mktemp)"; trap 'rm -f "$tmp" "$zip"' EXIT
for arg in "$@"; do
  if [ -f "$arg" ]; then
    sed -E $'s/\\x1b\\[[0-9;]*m//g' "$arg" >> "$tmp"
  else
    command -v unzip >/dev/null 2>&1 \
      || { echo "ci-refresh-weights: unzip is required to read a run id's log ZIP (install unzip); not falling back to the truncating \`gh run view --log\`" >&2; exit 1; }
    gh api "repos/${CI_REFRESH_REPO:-j0yen/mcphost}/actions/runs/$arg/logs" > "$zip" 2>/dev/null \
      && unzip -p "$zip" | sed -E $'s/\\x1b\\[[0-9;]*m//g' >> "$tmp" \
      || { echo "ci-refresh-weights: cannot fetch the log ZIP of run $arg" >&2; exit 1; }
  fi
done

python3 - "$tmp" <<'PY'
import glob, json, os, re, sys

LINE = re.compile(r'\bsuite=(\S+) measured=([0-9.]+)s\b')
SOLO = re.compile(r'^//!\s*solo-suite:\s*([0-9]+)\b')
measured = {}
for line in open(sys.argv[1], encoding="utf-8", errors="replace"):
    for name, secs in LINE.findall(line):
        measured[name] = max(measured.get(name, 0.0), float(secs))
measured = {k: v for k, v in measured.items() if not k.startswith("bin:")}
if not measured:
    sys.exit("ci-refresh-weights: no `suite=<name> measured=<s>s` lines in the input")

W, M = "scripts/ci-suite-weights.json", "scripts/ci-suite-measured.json"
weights = json.load(open(W))

# COMPLETENESS (generated from the weights file, never a hand list): every core suite must be
# measured in the input, else a truncated log silently drops suites from the measured file.
core = [k for k in weights if k != "lib" and not k.startswith("bin:") and "sandbox" not in k]
missing = sorted(k for k in core if k not in measured)
if missing:
    sys.exit("ci-refresh-weights: input lacks a `suite=<name> measured=` line for core suite(s): "
             + ", ".join(missing) + " (truncated log? nothing written)")
old_m = json.load(open(M)) if os.path.exists(M) else {}

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
json.dump(dict(sorted(weights.items())), open(W, "w"), indent=2); open(W, "a").write("\n")
json.dump(dict(sorted(new_m.items())), open(M, "w"), indent=2); open(M, "a").write("\n")
print(f"ci-refresh-weights: wrote {M} ({len(new_m)} suites) and {W}")
PY
