#!/usr/bin/env bash
# sandbox-api-doc-check.sh -- PRD-mcphost-sandbox-bridge-discoverability
# requirement 7 (AC7): docs/kinds/python.md, www/llms.txt, and
# plugin/skills/mcphost/SKILL.md must each carry the same module table --
# one bullet per `mcphost.*` submodule python-kind tool code can import,
# generated from src/kinds/python.rs's own BRIDGE_MODULES constant (the
# registration constant requirement 1's host.quickstart sandbox_api field
# also reads from -- see kinds::python::build_sandbox_api) rather than a
# second, hand-copied list that can drift from it.
#
# Usage:
#   scripts/sandbox-api-doc-check.sh          check: exit 0 and report
#                                              modules=<n> docs_in_sync=<n>,
#                                              or exit 1 naming the first
#                                              missing module and file.
#   scripts/sandbox-api-doc-check.sh --write  regenerate the marked block
#                                              in every target file.
set -uo pipefail
cd "$(dirname "$0")/.."

SOURCE="src/kinds/python.rs"
TARGETS=("docs/kinds/python.md" "docs/kinds/js.md" "www/llms.txt" "plugin/skills/mcphost/SKILL.md")
START="<!-- mcphost-sandbox-api:start -->"
END="<!-- mcphost-sandbox-api:end -->"
MODE="${1:-check}"
[ "$MODE" = "--write" ] && MODE="write"

if [ ! -f "$SOURCE" ]; then
  echo "sandbox-api-doc-check.sh: missing $SOURCE" >&2
  exit 2
fi

# Renders the marked block's own content (markers included) from
# BRIDGE_MODULES's own `name: "..."` / `purpose: "..."` pairs, parsed back
# out of the Rust source text -- never a second, hand-maintained list.
render_block() {
  python3 - "$SOURCE" "$START" "$END" <<'PY'
import re
import sys

source_path, start, end = sys.argv[1:4]
with open(source_path, encoding="utf-8") as f:
    text = f.read()

m = re.search(r"pub const BRIDGE_MODULES: &\[BridgeModule\] = &\[(.*?)\n\];", text, re.S)
if not m:
    print("sandbox-api-doc-check.sh: could not find BRIDGE_MODULES in " + source_path, file=sys.stderr)
    sys.exit(2)
block = m.group(1)

names = re.findall(r'name: "([a-z_]+)"', block)
purposes = re.findall(r'purpose: "([^"]*)"', block)
if not names or len(names) != len(purposes):
    print(
        f"sandbox-api-doc-check.sh: BRIDGE_MODULES parse mismatch: {len(names)} names, "
        f"{len(purposes)} purposes",
        file=sys.stderr,
    )
    sys.exit(2)

print(start)
print(
    "A python tool's own code does `import mcphost` to reach this tenant's "
    "data without a second tool call:"
)
print()
for name, purpose in zip(names, purposes):
    print(f"- `mcphost.{name}` -- {purpose}")
print(end)
PY
}

# Replaces the text between START and END (inclusive) in $1 with $2 --
# same mechanism gen-agent-docs.sh/gen-docs-sharing.sh already use. Fails
# loudly if the file has zero or more than one start/end marker pair.
apply_to_file() {
  local file="$1" rendered="$2"
  local n_start n_end
  n_start=$(grep -Fc "$START" "$file")
  n_end=$(grep -Fc "$END" "$file")
  if [ "$n_start" -ne 1 ] || [ "$n_end" -ne 1 ]; then
    echo "sandbox-api-doc-check.sh: $file must have exactly one $START / $END pair (found start=$n_start end=$n_end)" >&2
    return 2
  fi
  case "$rendered" in
    "$START"*"$END") ;;
    *)
      echo "sandbox-api-doc-check.sh: refusing to write $file: the rendered block is empty or lacks its $START / $END markers" >&2
      return 2
      ;;
  esac
  local tmp
  tmp="$(mktemp "$file.XXXXXX")"  # same dir: mv is an atomic rename, no unlink window
  if ! python3 - "$file" "$START" "$END" "$rendered" > "$tmp" <<'PY'
import sys

file_path, start_marker, end_marker, rendered = sys.argv[1:5]
with open(file_path, encoding="utf-8") as f:
    text = f.read()
start_idx = text.index(start_marker)
end_idx = text.index(end_marker) + len(end_marker)
if end_idx <= start_idx:
    sys.exit("end marker precedes start marker")
sys.stdout.write(text[:start_idx] + rendered + text[end_idx:])
PY
  then
    rm -f "$tmp"
    echo "sandbox-api-doc-check.sh: refusing to write $file: the marker splice failed; file left untouched" >&2
    return 2
  fi
  if [ ! -s "$tmp" ]; then
    rm -f "$tmp"
    echo "sandbox-api-doc-check.sh: refusing to write $file: the splice produced an empty file; file left untouched" >&2
    return 2
  fi
  chmod 644 "$tmp"
  mv "$tmp" "$file"
}

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
      echo "sandbox-api-doc-check.sh: $label output is empty or lacks its $CAP_START / $CAP_END markers; writing nothing" >&2
      return 2
      ;;
  esac
  rows=$(grep -cE '^\| `[a-z_]+` \| (yes|no) \|' <<<"$block" || true)
  if [ "${rows:-0}" -lt 1 ]; then
    echo "sandbox-api-doc-check.sh: $label output has no capability rows; writing nothing" >&2
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
    echo "sandbox-api-doc-check.sh: capabilities-section --doc disagrees with capabilities-section or lacks its title heading; writing nothing" >&2
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
    echo "sandbox-api-doc-check.sh: refusing to write www/llms.txt: the capabilities splice failed; file left untouched" >&2
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
RECIPE_START="<!-- recipe:sub_agent_key -->"
RECIPE_END="<!-- /recipe:sub_agent_key -->"

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
    *) echo "sandbox-api-doc-check.sh: recipe-section output lacks its markers; writing nothing" >&2; return 2 ;;
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
    echo "sandbox-api-doc-check.sh: refusing to write README.md: the recipe splice failed; file left untouched" >&2
    return 2
  fi
  if [ ! -s "$tmp" ]; then
    rm -f "$tmp"
    echo "sandbox-api-doc-check.sh: refusing to write README.md: the splice produced an empty file; file left untouched" >&2
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
      echo "sandbox-api-doc-check.sh: prompts-section did not render a usable $PROMPTS_DOC; writing nothing" >&2
      return 2
      ;;
  esac
  tmp="$(mktemp "$PROMPTS_DOC.XXXXXX")" || return $?
  printf '%s\n' "$doc" > "$tmp" && chmod 644 "$tmp" && mv "$tmp" "$PROMPTS_DOC"
}

RENDERED="$(render_block)" || exit $?
MODULE_COUNT=$(printf '%s\n' "$RENDERED" | grep -c '^- `mcphost\.')

if [ "$MODE" = "write" ]; then
  for f in "${TARGETS[@]}"; do
    apply_to_file "$f" "$RENDERED" || exit $?
  done
  write_capabilities || exit $?
  write_recipe_readme || exit $?
  write_prompts_doc || exit $?
  echo "sandbox-api-doc-check.sh: wrote modules=${MODULE_COUNT} to ${#TARGETS[@]} files"
  # PRD-mcphost-dry-run-on-write-verbs R5: docs/host-tools.md, generated
  # from src/write_verbs.rs, rides along in both modes.
  "$(dirname "$0")/host-tools-doc-check.sh" --write || exit $?
  # PRD-mcphost-sse-frame-contract-generated: frames.md/frames.json and the
  # llms.txt Transport block, generated from FRAME_SEQUENCES.
  "$(dirname "$0")/protocol-frames-check.sh" --write || exit $?
  exit 0
fi

# Check mode: each target's own marked block must equal $RENDERED exactly;
# a drifted or missing module is named with the file it's missing from.
FAILURES=0
for f in "${TARGETS[@]}"; do
  if [ ! -f "$f" ]; then
    echo "sandbox-api-doc-check.sh: $f does not exist" >&2
    FAILURES=$((FAILURES + 1))
    continue
  fi
  n_start=$(grep -Fc "$START" "$f")
  n_end=$(grep -Fc "$END" "$f")
  if [ "$n_start" -ne 1 ] || [ "$n_end" -ne 1 ]; then
    echo "sandbox-api-doc-check.sh: $f must have exactly one $START / $END pair (found start=$n_start end=$n_end)" >&2
    FAILURES=$((FAILURES + 1))
    continue
  fi
  ACTUAL=$(python3 - "$f" "$START" "$END" <<'PY'
import sys

file_path, start_marker, end_marker = sys.argv[1:4]
with open(file_path, encoding="utf-8") as f:
    text = f.read()
start_idx = text.index(start_marker)
end_idx = text.index(end_marker) + len(end_marker)
sys.stdout.write(text[start_idx:end_idx])
PY
  )
  while IFS= read -r name; do
    # A here-string, not `printf | grep -q`: grep -q exits on its first match
    # and, under `pipefail`, a printf still writing its trailing newline gets
    # SIGPIPE and fails the pipeline -- a false "is missing" about 1 run in 100
    # under load.
    if ! grep -qF "\`mcphost.${name}\`" <<<"$ACTUAL"; then
      echo "sandbox-api-doc-check.sh: $f is missing mcphost.${name}" >&2
      FAILURES=$((FAILURES + 1))
    fi
  done < <(printf '%s\n' "$RENDERED" | grep -o '`mcphost\.[a-z_]*`' | sed -e 's/`mcphost\.//' -e 's/`//')
done

CAP_EXPECTED="$(mcphost_bin capabilities-section)" || {
  echo "sandbox-api-doc-check.sh: could not render the capabilities section (mcphost capabilities-section failed)" >&2
  exit 2
}
for f in "${CAP_TARGETS[@]}"; do
  if [ ! -f "$f" ]; then
    echo "sandbox-api-doc-check.sh: $f does not exist (run sandbox-api-doc-check.sh --write)" >&2
    FAILURES=$((FAILURES + 1))
    continue
  fi
  CAP_ACTUAL="$(cap_block_of "$f")"
  if [ "$CAP_ACTUAL" != "$CAP_EXPECTED" ]; then
    echo "sandbox-api-doc-check.sh: $f ## Capabilities section differs from render_capabilities_section() (run sandbox-api-doc-check.sh --write)" >&2
    FAILURES=$((FAILURES + 1))
  fi
done

RECIPE_EXPECTED="$(mcphost_bin recipe-section)" || {
  echo "sandbox-api-doc-check.sh: could not render the sub-agent recipe (mcphost recipe-section failed)" >&2
  exit 2
}
RECIPE_ACTUAL="$(recipe_block_of "$README_FILE")"
if [ "$RECIPE_ACTUAL" != "$RECIPE_EXPECTED" ]; then
  echo "sandbox-api-doc-check.sh: $README_FILE recipe:sub_agent_key block differs from the rendered recipe (run sandbox-api-doc-check.sh --write)" >&2
  FAILURES=$((FAILURES + 1))
fi

PROMPTS_EXPECTED="$(mcphost_bin prompts-section)" || {
  echo "sandbox-api-doc-check.sh: could not render the prompts table (mcphost prompts-section failed)" >&2
  exit 2
}
if [ ! -f "$PROMPTS_DOC" ]; then
  echo "sandbox-api-doc-check.sh: $PROMPTS_DOC does not exist (run sandbox-api-doc-check.sh --write)" >&2
  FAILURES=$((FAILURES + 1))
elif [ "$(cat "$PROMPTS_DOC")" != "$PROMPTS_EXPECTED" ]; then
  echo "sandbox-api-doc-check.sh: $PROMPTS_DOC differs from recipe_prompts() rendered (run sandbox-api-doc-check.sh --write)" >&2
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

# PRD-mcphost-oauth-first-entry-url requirement 1: the OAuth-first entry path
# is the one constant `OAUTH_ENTRY_PATH` in src/oauth.rs. Every client page
# that names the entry URL must carry exactly that path (nothing else under
# /login/), and the install docs must name it at all.
ENTRY_PATH=$(sed -n 's|^pub const OAUTH_ENTRY_PATH: &str = "\(.*\)";|\1|p' src/oauth.rs)
if [ -z "$ENTRY_PATH" ]; then
  echo "sandbox-api-doc-check.sh: could not read OAUTH_ENTRY_PATH from src/oauth.rs" >&2
  FAILURES=$((FAILURES + 1))
else
  for f in README.md llms-install.md www/llms-install.md www/llms.txt www/llms-full.txt www/index.html docs/oauth-connect.md; do
    if ! grep -qF "https://mcphost.dev${ENTRY_PATH}" "$f"; then
      echo "sandbox-api-doc-check.sh: $f does not name the OAuth entry URL https://mcphost.dev${ENTRY_PATH}" >&2
      FAILURES=$((FAILURES + 1))
    fi
    stray=$(grep -oE 'https://mcphost\.dev/login/[A-Za-z0-9_./-]*' "$f" | grep -vxF "https://mcphost.dev${ENTRY_PATH}" || true)
    if [ -n "$stray" ]; then
      echo "sandbox-api-doc-check.sh: $f names an entry path other than ${ENTRY_PATH}: $stray" >&2
      FAILURES=$((FAILURES + 1))
    fi
  done
fi

if [ "$FAILURES" -gt 0 ]; then
  echo "sandbox-api-doc-check.sh: FAILED (${FAILURES} issue(s))" >&2
  exit 1
fi

echo "modules=${MODULE_COUNT} docs_in_sync=${#TARGETS[@]}"
exit 0
