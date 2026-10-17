#!/usr/bin/env bash
# Fail (exit 1) when the release-bumped files disagree with each other.
#
# The `static analysis + core suites` job skips its heavy suites when the
# normalized code tree already passed (scripts/ci/normalized-tree-hash.sh swaps
# the version files out of the hash). The contract tests that read those files
# are skipped on such a hit, so this cheap check runs on EVERY run (hit and
# miss) and proves what the skipped tests would have:
#   1. plugin/.claude-plugin/plugin.json and plugin/.mcp.json (and a root
#      plugin.json / .mcp.json where they exist) carry the Cargo.toml
#      [package] version;
#   2. every `## ` heading of CHANGELOG.md is `## v<semver> — <date>`;
#   3. the headings are strictly descending;
#   4. the top heading is on Cargo's major.minor line and not ahead of it.
#
# Usage: scripts/ci/version-files-consistency.sh [repo-root]   (default: repo of this script)
set -euo pipefail

root=${1:-$(cd "$(dirname "$0")/../.." && pwd)}
cd "$root"
fail=0
bad() { echo "version-files-consistency: $*" >&2; fail=1; }

# [package] version from Cargo.toml
cargo=$(awk '/^\[/ { sec = $0 } sec == "[package]" && /^version[ \t]*=/ { gsub(/^[^"]*"|".*$/, ""); print; exit }' Cargo.toml)
if ! printf '%s\n' "$cargo" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
  echo "version-files-consistency: cannot read a semver [package] version from Cargo.toml (got '$cargo')" >&2
  exit 1
fi

for f in plugin/.claude-plugin/plugin.json plugin/.mcp.json plugin.json .mcp.json; do
  [ -f "$f" ] || continue
  v=$(sed -nE 's/^[[:space:]]*"version"[[:space:]]*:[[:space:]]*"([^"]*)".*/\1/p' "$f" | head -n1)
  [ -n "$v" ] || continue
  [ "$v" = "$cargo" ] || bad "$f version $v != Cargo.toml version $cargo"
done

if [ -f CHANGELOG.md ]; then
  heads=$(grep '^## ' CHANGELOG.md || true)
  if [ -z "$heads" ]; then
    bad "CHANGELOG.md has no '## v<semver> — <date>' headings"
  else
    while IFS= read -r h; do
      printf '%s\n' "$h" | grep -Eq '^## v[0-9]+\.[0-9]+\.[0-9]+ — [0-9]{4}-[0-9]{2}-[0-9]{2}$' \
        || bad "CHANGELOG.md heading is not '## v<semver> — <date>': $h"
    done <<< "$heads"
    versions=$(printf '%s\n' "$heads" | sed -nE 's/^## v([0-9]+\.[0-9]+\.[0-9]+) .*/\1/p')
    if [ -n "$versions" ]; then
      printf '%s\n' "$versions" | awk -F. -v cargo="$cargo" '
        function cmp(a, b,   x, y, i) {
          split(a, x, "."); split(b, y, ".")
          for (i = 1; i <= 3; i++) { if (x[i] + 0 != y[i] + 0) return (x[i] + 0 < y[i] + 0) ? -1 : 1 }
          return 0
        }
        NR == 1 { top = $0 }
        { v = $1 "." $2 "." $3 }
        NR > 1 && cmp(v, prev) >= 0 { printf "version-files-consistency: CHANGELOG.md versions not strictly descending: v%s then v%s\n", prev, v > "/dev/stderr"; bad = 1 }
        { prev = v }
        END {
          split(top, t, "."); split(cargo, c, ".")
          if (t[1] != c[1] || t[2] != c[2]) { printf "version-files-consistency: CHANGELOG.md top v%s is not on Cargo.toml %s major.minor line\n", top, cargo > "/dev/stderr"; bad = 1 }
          else if (cmp(top, cargo) > 0) { printf "version-files-consistency: CHANGELOG.md top v%s is ahead of Cargo.toml %s\n", top, cargo > "/dev/stderr"; bad = 1 }
          exit bad
        }' || fail=1
    fi
  fi
fi

if [ "$fail" -ne 0 ]; then exit 1; fi
echo "version-files-consistency: ok (Cargo.toml $cargo)"
