#!/usr/bin/env bash
# Proof of scripts/ci/version-files-consistency.sh on scratch trees (no git
# needed). The CI gate job runs this before the check itself.
set -euo pipefail

script=$(cd "$(dirname "$0")" && pwd)/version-files-consistency.sh
scratch_root=${NTREE_TEST_ROOT:-${RUNNER_TEMP:-/tmp}}
mkdir -p "$scratch_root"
work=$(mktemp -d "$scratch_root/vfc-test.XXXXXX")
trap 'rm -rf "$work"' EXIT

pass=0
fail=0
ok() { pass=$((pass + 1)); echo "PASS  $1"; }
bad() { fail=$((fail + 1)); echo "FAIL  $1"; }
expect_ok() { if bash "$script" "$work" >/dev/null 2>&1; then ok "$1"; else bad "$1 (expected ok)"; fi; }
expect_fail() { # <name> <stderr substring>
  local out
  if out=$(bash "$script" "$work" 2>&1); then bad "$1 (expected failure, got ok)"
  elif printf '%s' "$out" | grep -qF -- "$2"; then ok "$1"
  else bad "$1 (failed without '$2': $out)"; fi
}

reset_tree() {
  find "$work" -mindepth 1 -delete
  mkdir -p "$work/plugin/.claude-plugin"
  printf '[package]\nname = "demo"\nversion = "1.2.3"\n\n[dependencies]\nserde = { version = "1" }\n' > "$work/Cargo.toml"
  cat > "$work/Cargo.lock" <<LEOF
version = 4

[[package]]
name = "demo"
version = "1.2.3"
dependencies = ["serde"]

[[package]]
name = "serde"
version = "1.0.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "aaaa"
LEOF
  printf '{\n  "name": "demo",\n  "version": "1.2.3"\n}\n' > "$work/plugin/.claude-plugin/plugin.json"
  printf '{\n  "mcpServers": {},\n  "version": "1.2.3"\n}\n' > "$work/plugin/.mcp.json"
  cat > "$work/CHANGELOG.md" <<CLEOF
# Changelog

## v1.2.2 — 2026-10-02

Top.

## v1.2.0 — 2026-10-01

Older.

## v1.1.9 — 2026-09-30

Oldest.
CLEOF
}

reset_tree
expect_ok "consistent tree passes"

reset_tree
sed -i 's/"version": "1.2.3"/"version": "1.2.4"/' "$work/plugin/.claude-plugin/plugin.json"
expect_fail "plugin.json version mismatch is caught" "plugin/.claude-plugin/plugin.json version 1.2.4 != Cargo.toml version 1.2.3"

reset_tree
sed -i 's/"version": "1.2.3"/"version": "1.2.0"/' "$work/plugin/.mcp.json"
expect_fail ".mcp.json version mismatch is caught" "plugin/.mcp.json version 1.2.0"

reset_tree
printf '{\n  "version": "9.9.9"\n}\n' > "$work/plugin.json"
expect_fail "root plugin.json version mismatch is caught" "plugin.json version 9.9.9"

reset_tree
rm -rf "$work/plugin"
expect_ok "absent plugin files are skipped"

reset_tree
sed -i 's/^## v1.2.0 — 2026-10-01/## v1.2.0 - 2026-10-01/' "$work/CHANGELOG.md"
expect_fail "a heading with a hyphen instead of an em dash is caught" "is not '## v<semver> — <date>'"

reset_tree
printf '\n## Unreleased\n\nx\n' >> "$work/CHANGELOG.md"
expect_fail "a non-version heading is caught" "## Unreleased"

reset_tree
sed -i 's/^## v1.2.0 /## v1.2.5 /' "$work/CHANGELOG.md"
expect_fail "an ascending pair is caught" "not strictly descending: v1.2.2 then v1.2.5"

reset_tree
sed -i 's/^## v1.1.9 /## v1.2.0 /' "$work/CHANGELOG.md"
expect_fail "a duplicate version is caught" "not strictly descending"

reset_tree
sed -i 's/^## v1.2.2 /## v1.3.0 /; s/^## v1.2.0 /## v1.2.9 /' "$work/CHANGELOG.md"
expect_fail "a top heading on another minor line is caught" "not on Cargo.toml 1.2.3 major.minor line"

reset_tree
sed -i 's/^## v1.2.2 /## v1.2.4 /' "$work/CHANGELOG.md"
expect_fail "a top heading ahead of Cargo.toml is caught" "ahead of Cargo.toml"

reset_tree
sed -i 's/^version = "1.2.3"/version = "1.2.2"/' "$work/Cargo.toml"
sed -i 's/"version": "1.2.3"/"version": "1.2.2"/' "$work/plugin/.claude-plugin/plugin.json" "$work/plugin/.mcp.json"
sed -i '0,/^version = "1.2.3"/s//version = "1.2.2"/' "$work/Cargo.lock"
expect_ok "top heading equal to the Cargo version passes"

reset_tree
sed -i 's/^version = "1.2.3"/version = "dev"/' "$work/Cargo.toml"
expect_fail "an unreadable Cargo version fails loudly" "cannot read a semver"

# Cargo.lock must carry our package's version
reset_tree
sed -i '0,/^version = "1.2.3"/s//version = "1.2.2"/' "$work/Cargo.lock"
expect_fail "a stale Cargo.lock (Cargo.toml and plugin JSONs bumped past it) is caught" "Cargo.lock demo version 1.2.2 != Cargo.toml version 1.2.3"

reset_tree
sed -i 's/^name = "demo"$/name = "other"/' "$work/Cargo.lock"
expect_fail "a Cargo.lock without our package entry is caught" "Cargo.lock has no path-package entry for demo"

reset_tree
sed -i 's/^version = "1.0.1"/version = "1.2.3"/' "$work/Cargo.lock"
expect_ok "a registry package's version does not matter to the lock check"

reset_tree
sed -i 's/^version = "1.0.1"/version = "1.2.3"/; 0,/^version = "1.2.3"/s//version = "9.9.9"/' "$work/Cargo.lock"
expect_fail "only the path package's lock version counts, not a registry one" "Cargo.lock demo version 9.9.9"

reset_tree
rm -f "$work/Cargo.lock"
expect_ok "an absent Cargo.lock is skipped"

echo "version-files-consistency: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
