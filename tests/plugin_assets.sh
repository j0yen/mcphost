#!/usr/bin/env bash
# plugin_assets.sh -- PRD-mcphost-claude-code-plugin-and-snippets, AC1/requirement 5.
#
# Asserts, printing one line per check: every plugin/README/www file this
# PRD adds exists, both plugin JSON files parse with jq, the plugin
# version matches Cargo.toml's, every connect snippet names the mcphost
# endpoint, SKILL.md carries the claim/source contract, and no snippet
# leaks a real bearer key.
set -uo pipefail
cd "$(dirname "$0")/.."

status=0
URL="https://mcphost.dev/mcp"

check() {
  local desc="$1"
  shift
  if "$@"; then
    echo "ok - $desc"
  else
    echo "FAIL - $desc"
    status=1
  fi
}

file_exists() { [ -f "$1" ]; }
grep_has() { grep -qF -- "$2" "$1"; }

# jq is the PRD's own tool of choice (requirement 5); fall back to
# python3's stdlib json module (always available, same precedent as
# scripts/www-check.sh) on a box that doesn't have jq installed --
# this script must exit 0 on the built tree either way.
json_field() {
  local file="$1" field="$2"
  if command -v jq >/dev/null 2>&1; then
    jq -r ".$field" "$file" 2>/dev/null
  else
    python3 -c "import json,sys; print(json.load(open(sys.argv[1])).get(sys.argv[2], ''))" "$file" "$field" 2>/dev/null
  fi
}
jq_parses() {
  if command -v jq >/dev/null 2>&1; then
    jq -e . "$1" >/dev/null 2>&1
  else
    python3 -c "import json,sys; json.load(open(sys.argv[1]))" "$1" >/dev/null 2>&1
  fi
}

check "plugin/.claude-plugin/plugin.json exists" file_exists plugin/.claude-plugin/plugin.json
check "plugin/.mcp.json exists" file_exists plugin/.mcp.json
check "plugin/skills/mcphost/SKILL.md exists" file_exists plugin/skills/mcphost/SKILL.md
check "README.md exists" file_exists README.md
check "www/index.html exists" file_exists www/index.html
check "www/llms.txt exists" file_exists www/llms.txt

check "plugin/.claude-plugin/plugin.json parses with jq" jq_parses plugin/.claude-plugin/plugin.json
check "plugin/.mcp.json parses with jq" jq_parses plugin/.mcp.json

name_is_mcphost() { [ "$(json_field plugin/.claude-plugin/plugin.json name)" = "mcphost" ]; }
check "plugin.json .name == \"mcphost\"" name_is_mcphost

cargo_version="$(grep -m1 '^version' Cargo.toml | sed -E 's/version *= *"([^"]+)"/\1/')"
version_matches_cargo_toml() { [ "$(json_field plugin/.claude-plugin/plugin.json version)" = "$cargo_version" ]; }
check "plugin.json version ($cargo_version) matches Cargo.toml" version_matches_cargo_toml

check "README.md Connect snippets name $URL" grep_has README.md "$URL"
check "www/index.html snippets name $URL" grep_has www/index.html "$URL"
check "www/llms.txt walk-through names $URL" grep_has www/llms.txt "$URL"

check "SKILL.md contains claim_url" grep_has plugin/skills/mcphost/SKILL.md "claim_url"
check "SKILL.md tags signup with source: \"plugin\"" grep_has plugin/skills/mcphost/SKILL.md 'source: "plugin"'

no_leaked_key() {
  ! grep -rEo 'Bearer [A-Za-z0-9+/=_-]{40,}' plugin README.md www/index.html www/llms.txt 2>/dev/null | grep -q .
}
check "no snippet leaks a real bearer key" no_leaked_key

exit "$status"
