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
#   ci-refresh-weights.sh <log-file|run-id>...   # a run-id is fetched with
#                                                  `gh run view <id> -R j0yen/mcphost --log`
#   then: scripts/gen-test-suites.sh && scripts/ci-test-partition.sh check
# Env: CI_REFRESH_REPO (default j0yen/mcphost); CI_REFRESH_ROOT (default this checkout; tests use a scratch root).
set -uo pipefail
cd "${CI_REFRESH_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}" || exit 2
[ "$#" -ge 1 ] || { sed -n '2,20p' "$0" >&2; exit 2; }

tmp="$(mktemp)"; trap 'rm -f "$tmp"' EXIT
for arg in "$@"; do
  if [ -f "$arg" ]; then
    sed -E $'s/\\x1b\\[[0-9;]*m//g' "$arg" >> "$tmp"
  else
    gh run view "$arg" -R "${CI_REFRESH_REPO:-j0yen/mcphost}" --log 2>/dev/null | sed -E $'s/\\x1b\\[[0-9;]*m//g' >> "$tmp" \
      || { echo "ci-refresh-weights: cannot fetch run $arg" >&2; exit 1; }
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
