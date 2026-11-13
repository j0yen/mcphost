#!/usr/bin/env bash
# generated-docs-check.sh -- the doc blocks no `mcphost gen-docs` target renders
# yet. PRD-mcphost-kind-docs-from-registry-b-v2 R4: this was the umbrella
# `generated-docs-check` script; the mcphost-sandbox-api block moved into
# `mcphost gen-docs kinds` (the python kind's KindDoc.bridge_modules), and
# the checks below stay here, unchanged:
#   - docs/sandbox-api.md's bridge-modules block (`mcphost bridge-docs-block`)
#   - the `## Capabilities` section of www/llms.txt and docs/protocol/capabilities.md
#   - README.md's `quickstart-recipe:sub_agent_key` block
#   - docs/protocol/prompts.md
#   - scripts/protocol-frames-check.sh, scripts/host-tools-doc-check.sh,
#     scripts/signup-args-doc-check.sh
#   - the OAuth entry-path check over seven client files
#
# Usage:
#   scripts/generated-docs-check.sh          check: exit 0 and print
#                                              generated_docs_in_sync, or exit 1
#                                              naming every drifted file.
#   scripts/generated-docs-check.sh --write  regenerate every block above.
#   scripts/generated-docs-check.sh --write-bridge
#                                              regenerate only docs/sandbox-api.md's
#                                              bridge-modules block.
set -uo pipefail
cd "$(dirname "$0")/.."

MODE="${1:-check}"
[ "$MODE" = "--write" ] && MODE="write"
[ "$MODE" = "--write-bridge" ] && MODE="write-bridge"

# PRD-mcphost-server-capabilities-from-registry requirement 4: the
# `## Capabilities` section in www/llms.txt and docs/protocol/capabilities.md
# is `mcphost capabilities-section` (llms_txt::render_capabilities_section)
# byte-for-byte, so the CAPABILITIES registry stays the only place a
# primitive is named. Binary: $MCPHOST_BIN, else $CARGO_BIN_EXE_mcphost (set
# under `cargo test`), else a prebuilt target/{debug,release}/mcphost newer
# than src/ (under ./target, else under $CARGO_TARGET_DIR when set -- the
# daemon's worktrees build into a warm target dir outside the tree), else
# `cargo run`.
CAP_START="<!-- capabilities:start -->"
CAP_END="<!-- capabilities:end -->"
CAP_TARGETS=("www/llms.txt" "docs/protocol/capabilities.md")

mcphost_bin() {
  local bin="${MCPHOST_BIN:-${CARGO_BIN_EXE_mcphost:-}}"
  if [ -n "$bin" ]; then
    "$bin" "$@"
    return $?
  fi
  local cand dir
  for dir in target ${CARGO_TARGET_DIR:+"$CARGO_TARGET_DIR"}; do
    for cand in "$dir/release/mcphost" "$dir/debug/mcphost"; do
      if [ -x "$cand" ] && [ -z "$(find src Cargo.toml Cargo.lock -newer "$cand" -print -quit 2>/dev/null)" ]; then
        "$cand" "$@"
        return $?
      fi
    done
  done
  cargo run -q --bin mcphost -- "$@"
}

# Seed SDB: docs/sandbox-api.md's `mcphost-bridge-modules` block is
# `mcphost bridge-docs-block` (kinds::python::render_bridge_docs_block) byte
# for byte, so a new BRIDGE_MODULES entry can never leave the doc stale (the
# msmp_ac07 test compares the same two strings). The marker strings are read
# back off the rendered block (first and last line), never copied here.
BRIDGE_DOC="${MCPHOST_SANDBOX_DOC:-docs/sandbox-api.md}"

bridge_block_of() {
  python3 - "$1" "$2" "$3" <<'PY'
import sys

path, start, end = sys.argv[1:4]
with open(path, encoding="utf-8") as f:
    text = f.read()
if text.count(start) != 1 or text.count(end) != 1:
    sys.exit(3)
sys.stdout.write(text[text.index(start) : text.index(end) + len(end)])
PY
}

# Renders the bridge block and validates it (non-empty, wrapped in its own
# markers, at least one module bullet) before anything reads or writes a file.
render_bridge_block() {
  local block first last
  block="$(mcphost_bin bridge-docs-block)" || return $?
  first="${block%%$'\n'*}"
  last="${block##*$'\n'}"
  case "$first|$last" in
    "<!--"*":start -->|<!--"*":end -->") ;;
    *)
      echo "generated-docs-check.sh: bridge-docs-block output is empty or lacks its start/end markers; writing nothing" >&2
      return 2
      ;;
  esac
  if ! grep -qE '^- `mcphost\.[a-z_]+` -- ' <<<"$block"; then
    echo "generated-docs-check.sh: bridge-docs-block output has no module bullets; writing nothing" >&2
    return 2
  fi
  printf '%s' "$block"
}

write_bridge_doc() {
  local block first last n_start n_end tmp
  block="$(render_bridge_block)" || return $?
  first="${block%%$'\n'*}"
  last="${block##*$'\n'}"
  n_start=$(grep -Fxc "$first" "$BRIDGE_DOC")
  n_end=$(grep -Fxc "$last" "$BRIDGE_DOC")
  if [ "$n_start" -ne 1 ] || [ "$n_end" -ne 1 ]; then
    echo "generated-docs-check.sh: $BRIDGE_DOC must have exactly one $first / $last pair (found start=$n_start end=$n_end)" >&2
    return 2
  fi
  tmp="$(mktemp "$BRIDGE_DOC.XXXXXX")" || return $?  # same dir: mv is an atomic rename
  if ! python3 - "$BRIDGE_DOC" "$first" "$last" "$block" > "$tmp" <<'PY'
import sys

path, start, end, block = sys.argv[1:5]
with open(path, encoding="utf-8") as f:
    text = f.read()
s, e = text.index(start), text.index(end) + len(end)
if e <= s:
    sys.exit("end marker precedes start marker")
sys.stdout.write(text[:s] + block + text[e:])
PY
  then
    rm -f "$tmp"
    echo "generated-docs-check.sh: refusing to write $BRIDGE_DOC: the marker splice failed; file left untouched" >&2
    return 2
  fi
  if [ ! -s "$tmp" ]; then
    rm -f "$tmp"
    echo "generated-docs-check.sh: refusing to write $BRIDGE_DOC: the splice produced an empty file; file left untouched" >&2
    return 2
  fi
  chmod 644 "$tmp"
  mv "$tmp" "$BRIDGE_DOC"
}

# The marked block (markers included) of file $1 on stdout.
cap_block_of() {
  python3 - "$1" "$CAP_START" "$CAP_END" <<'PY'
import sys

path, start, end = sys.argv[1:4]
with open(path, encoding="utf-8") as f:
    text = f.read()
if start not in text or end not in text:
    sys.exit(3)
sys.stdout.write(text[text.index(start) : text.index(end) + len(end)])
PY
}

# A rendered capabilities block is usable only if it is non-empty, wrapped in
# its own markers, and carries at least one capability row. Anything else
# (cargo output, a compile error, an empty render) must never reach a file.
validate_cap_block() {
  local label="$1" block="$2" rows
  case "$block" in
    "$CAP_START"*"$CAP_END") ;;
    *)
      echo "generated-docs-check.sh: $label output is empty or lacks its $CAP_START / $CAP_END markers; writing nothing" >&2
      return 2
      ;;
  esac
  rows=$(grep -cE '^\| `[a-z_]+` \| (yes|no) \|' <<<"$block" || true)
  if [ "${rows:-0}" -lt 1 ]; then
    echo "generated-docs-check.sh: $label output has no capability rows; writing nothing" >&2
    return 2
  fi
}

# Everything is rendered into a temp file OUTSIDE the tree and validated
# before any target is touched. Never `mcphost_bin ... > docs/protocol/capabilities.md`:
# the shell truncates the redirect target before the command runs, and with no
# prebuilt binary `cargo run` re-runs build.rs (rerun-if-changed=docs), which
# derives `enum DocAnchor` from the docs/ headings -- so the truncation deleted
# `## Capabilities`, the build failed (E0599, help.rs), and the file stayed
# empty (runs 847/830, 2026-10-15 21:01Z).
write_capabilities() {
  local section tmp_doc
  section="$(mcphost_bin capabilities-section)" || return $?
  validate_cap_block "capabilities-section" "$section" || return $?
  tmp_doc="$(mktemp "${TMPDIR:-/tmp}/capabilities-doc.XXXXXX")" || return $?
  local rc=0
  mcphost_bin capabilities-section --doc > "$tmp_doc" || rc=$?
  if [ "$rc" -ne 0 ]; then
    rm -f "$tmp_doc"
    return "$rc"
  fi
  local doc_block
  doc_block="$(cap_block_of "$tmp_doc")" || doc_block=""
  if ! validate_cap_block "capabilities-section --doc" "$doc_block"; then
    rm -f "$tmp_doc"
    return 2
  fi
  if [ "$doc_block" != "$section" ] || [ "$(head -c2 "$tmp_doc")" != "# " ]; then
    rm -f "$tmp_doc"
    echo "generated-docs-check.sh: capabilities-section --doc disagrees with capabilities-section or lacks its title heading; writing nothing" >&2
    return 2
  fi
  mkdir -p docs/protocol
  if ! python3 - www/llms.txt "$CAP_START" "$CAP_END" "$section" <<'PY'
import sys

path, start, end, section = sys.argv[1:5]
with open(path, encoding="utf-8") as f:
    text = f.read()
if start in text and end in text:
    s, e = text.index(start), text.index(end) + len(end)
    if e <= s:
        sys.exit("end marker precedes start marker")
    text = text[:s] + section + text[e:]
elif "\n## Contract\n" in text:
    i = text.index("\n## Contract\n")
    text = text[:i] + "\n" + section + "\n" + text[i:]
else:
    text = text.rstrip("\n") + "\n\n" + section + "\n"
with open(path, "w", encoding="utf-8") as f:
    f.write(text)
PY
  then
    rm -f "$tmp_doc"
    echo "generated-docs-check.sh: refusing to write www/llms.txt: the capabilities splice failed; file left untouched" >&2
    return 2
  fi
  cat "$tmp_doc" > docs/protocol/capabilities.md || rc=$?
  rm -f "$tmp_doc"
  return "$rc"
}

# PRD-mcphost-delegation-recipes-and-quickstart requirement 5: README.md's
# `### Sub-agents` block between the recipe markers is `mcphost recipe-section`
# (llms_txt::render_recipe_block) byte-for-byte. Check mode fails iff it differs
# (a missing block differs); --write splices it via `mcphost llms-txt` into a
# temp copy so a failed render never touches README.md.
README_FILE="${MCPHOST_README:-README.md}"
RECIPE_START="<!-- quickstart-recipe:sub_agent_key -->"
RECIPE_END="<!-- /quickstart-recipe:sub_agent_key -->"

recipe_block_of() {
  python3 - "$1" "$RECIPE_START" "$RECIPE_END" <<'PY'
import sys

path, start, end = sys.argv[1:4]
with open(path, encoding="utf-8") as f:
    text = f.read()
if start not in text or end not in text:
    sys.exit(3)
sys.stdout.write(text[text.index(start) : text.index(end) + len(end)])
PY
}

write_recipe_readme() {
  local block tmp
  block="$(mcphost_bin recipe-section)" || return $?
  case "$block" in
    "$RECIPE_START"*"$RECIPE_END") ;;
    *) echo "generated-docs-check.sh: recipe-section output lacks its markers; writing nothing" >&2; return 2 ;;
  esac
  tmp="$(mktemp "${TMPDIR:-/tmp}/readme-recipe.XXXXXX")" || return $?
  if ! python3 - "$README_FILE" "$RECIPE_START" "$RECIPE_END" "$block" "### Sub-agents" "## Contributing: naming a new" > "$tmp" <<'PY'
import sys

path, start, end, block, heading, anchor = sys.argv[1:7]
with open(path, encoding="utf-8") as f:
    text = f.read()
if start in text and end in text:
    s, e = text.index(start), text.index(end) + len(end)
    if e <= s:
        sys.exit("end marker precedes start marker")
    out = text[:s] + block + text[e:]
elif anchor in text:
    i = text.index(anchor)
    out = text[:i] + heading + "\n\n" + block + "\n\n" + text[i:]
else:
    out = text.rstrip("\n") + "\n\n" + heading + "\n\n" + block + "\n"
sys.stdout.write(out)
PY
  then
    rm -f "$tmp"
    echo "generated-docs-check.sh: refusing to write README.md: the recipe splice failed; file left untouched" >&2
    return 2
  fi
  if [ ! -s "$tmp" ]; then
    rm -f "$tmp"
    echo "generated-docs-check.sh: refusing to write README.md: the splice produced an empty file; file left untouched" >&2
    return 2
  fi
  cat "$tmp" > "$README_FILE"
  rm -f "$tmp"
}

# PRD-mcphost-prompts-from-published-tools requirement 5: docs/protocol/prompts.md
# is `mcphost prompts-section` (prompts::render_prompts_doc) and nothing else.
PROMPTS_DOC="docs/protocol/prompts.md"

write_prompts_doc() {
  local doc tmp
  doc="$(mcphost_bin prompts-section)" || return $?
  case "$doc" in
    "# MCP prompts"*"<!-- prompts:end -->") ;;
    *)
      echo "generated-docs-check.sh: prompts-section did not render a usable $PROMPTS_DOC; writing nothing" >&2
      return 2
      ;;
  esac
  tmp="$(mktemp "$PROMPTS_DOC.XXXXXX")" || return $?
  printf '%s\n' "$doc" > "$tmp" && chmod 644 "$tmp" && mv "$tmp" "$PROMPTS_DOC"
}

# --write-bridge: only docs/sandbox-api.md's bridge-modules block (what the
# msmp_ac07 test pins), none of the other generated docs.
if [ "$MODE" = "write-bridge" ]; then
  write_bridge_doc || exit $?
  echo "generated-docs-check.sh: wrote the bridge-modules block to $BRIDGE_DOC"
  exit 0
fi

if [ "$MODE" = "write" ]; then
  write_bridge_doc || exit $?
  write_capabilities || exit $?
  write_recipe_readme || exit $?
  write_prompts_doc || exit $?
  echo "generated-docs-check.sh: wrote the generated blocks"
  # PRD-mcphost-dry-run-on-write-verbs R5: docs/host-tools.md, generated
  # from src/write_verbs.rs, rides along in both modes.
  "$(dirname "$0")/host-tools-doc-check.sh" --write || exit $?
  # signup argument block in SKILL.md / www/skill.md, generated from
  # control::SIGNUP_ARGS (the descriptor's own source).
  "$(dirname "$0")/signup-args-doc-check.sh" --write || exit $?
  # PRD-mcphost-sse-frame-contract-generated: frames.md/frames.json and the
  # llms.txt Transport block, generated from FRAME_SEQUENCES.
  "$(dirname "$0")/protocol-frames-check.sh" --write || exit $?
  exit 0
fi

# Check mode: every drift is counted and named; the sandbox-API block of the
# kind pages, www/llms.txt and the plugin skill is `mcphost gen-docs kinds --check`.
FAILURES=0

BRIDGE_EXPECTED="$(render_bridge_block)" || {
  echo "generated-docs-check.sh: could not render the bridge-modules block (mcphost bridge-docs-block failed)" >&2
  exit 2
}
BRIDGE_FIRST="${BRIDGE_EXPECTED%%$'\n'*}"
BRIDGE_LAST="${BRIDGE_EXPECTED##*$'\n'}"
if [ ! -f "$BRIDGE_DOC" ]; then
  echo "generated-docs-check.sh: $BRIDGE_DOC does not exist (run generated-docs-check.sh --write)" >&2
  FAILURES=$((FAILURES + 1))
elif ! BRIDGE_ACTUAL="$(bridge_block_of "$BRIDGE_DOC" "$BRIDGE_FIRST" "$BRIDGE_LAST")"; then
  echo "generated-docs-check.sh: $BRIDGE_DOC must have exactly one $BRIDGE_FIRST / $BRIDGE_LAST pair" >&2
  FAILURES=$((FAILURES + 1))
elif [ "$BRIDGE_ACTUAL" != "$BRIDGE_EXPECTED" ]; then
  echo "generated-docs-check.sh: $BRIDGE_DOC bridge-modules block differs from render_bridge_docs_block() (run generated-docs-check.sh --write)" >&2
  FAILURES=$((FAILURES + 1))
fi

CAP_EXPECTED="$(mcphost_bin capabilities-section)" || {
  echo "generated-docs-check.sh: could not render the capabilities section (mcphost capabilities-section failed)" >&2
  exit 2
}
for f in "${CAP_TARGETS[@]}"; do
  if [ ! -f "$f" ]; then
    echo "generated-docs-check.sh: $f does not exist (run generated-docs-check.sh --write)" >&2
    FAILURES=$((FAILURES + 1))
    continue
  fi
  CAP_ACTUAL="$(cap_block_of "$f")"
  if [ "$CAP_ACTUAL" != "$CAP_EXPECTED" ]; then
    echo "generated-docs-check.sh: $f ## Capabilities section differs from render_capabilities_section() (run generated-docs-check.sh --write)" >&2
    FAILURES=$((FAILURES + 1))
  fi
done

RECIPE_EXPECTED="$(mcphost_bin recipe-section)" || {
  echo "generated-docs-check.sh: could not render the sub-agent recipe (mcphost recipe-section failed)" >&2
  exit 2
}
RECIPE_ACTUAL="$(recipe_block_of "$README_FILE")"
if [ "$RECIPE_ACTUAL" != "$RECIPE_EXPECTED" ]; then
  echo "generated-docs-check.sh: $README_FILE quickstart-recipe:sub_agent_key block differs from the rendered recipe (run generated-docs-check.sh --write)" >&2
  FAILURES=$((FAILURES + 1))
fi

PROMPTS_EXPECTED="$(mcphost_bin prompts-section)" || {
  echo "generated-docs-check.sh: could not render the prompts table (mcphost prompts-section failed)" >&2
  exit 2
}
if [ ! -f "$PROMPTS_DOC" ]; then
  echo "generated-docs-check.sh: $PROMPTS_DOC does not exist (run generated-docs-check.sh --write)" >&2
  FAILURES=$((FAILURES + 1))
elif [ "$(cat "$PROMPTS_DOC")" != "$PROMPTS_EXPECTED" ]; then
  echo "generated-docs-check.sh: $PROMPTS_DOC differs from recipe_prompts() rendered (run generated-docs-check.sh --write)" >&2
  FAILURES=$((FAILURES + 1))
fi

if ! "$(dirname "$0")/protocol-frames-check.sh" --check 2>/dev/null; then
  "$(dirname "$0")/protocol-frames-check.sh" --check >&2
  FAILURES=$((FAILURES + 1))
fi

if ! "$(dirname "$0")/host-tools-doc-check.sh" >/dev/null; then
  "$(dirname "$0")/host-tools-doc-check.sh" >&2
  FAILURES=$((FAILURES + 1))
fi

# signup argument block (control::SIGNUP_ARGS) in plugin/skills/mcphost/SKILL.md
# and www/skill.md.
if ! "$(dirname "$0")/signup-args-doc-check.sh" >/dev/null; then
  "$(dirname "$0")/signup-args-doc-check.sh" >&2
  FAILURES=$((FAILURES + 1))
fi

# PRD-mcphost-oauth-first-entry-url requirement 1: the OAuth-first entry path
# is the one constant `OAUTH_ENTRY_PATH` in src/oauth.rs. Every client page
# that names the entry URL must carry exactly that path (nothing else under
# /login/), and the install docs must name it at all.
ENTRY_PATH=$(sed -n 's|^pub const OAUTH_ENTRY_PATH: &str = "\(.*\)";|\1|p' src/oauth.rs)
if [ -z "$ENTRY_PATH" ]; then
  echo "generated-docs-check.sh: could not read OAUTH_ENTRY_PATH from src/oauth.rs" >&2
  FAILURES=$((FAILURES + 1))
else
  for f in README.md llms-install.md www/llms-install.md www/llms.txt www/llms-full.txt www/index.html docs/oauth-connect.md; do
    if ! grep -qF "https://mcphost.dev${ENTRY_PATH}" "$f"; then
      echo "generated-docs-check.sh: $f does not name the OAuth entry URL https://mcphost.dev${ENTRY_PATH}" >&2
      FAILURES=$((FAILURES + 1))
    fi
    stray=$(grep -oE 'https://mcphost\.dev/login/[A-Za-z0-9_./-]*' "$f" | grep -vxF "https://mcphost.dev${ENTRY_PATH}" || true)
    if [ -n "$stray" ]; then
      echo "generated-docs-check.sh: $f names an entry path other than ${ENTRY_PATH}: $stray" >&2
      FAILURES=$((FAILURES + 1))
    fi
  done
fi

if [ "$FAILURES" -gt 0 ]; then
  echo "generated-docs-check.sh: FAILED (${FAILURES} issue(s))" >&2
  exit 1
fi

echo "generated_docs_in_sync"
exit 0
