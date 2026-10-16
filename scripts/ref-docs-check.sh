#!/usr/bin/env bash
# ref-docs-check.sh -- the reference-doc drift checks that used to ride along
# in scripts/ref-docs-check.sh (PRD-mcphost-kind-docs-from-registry-b
# folded that script's python sandbox-API block into `mcphost gen-docs kinds`
# and deleted it): the `## Capabilities` section, docs/host-tools.md, the
# SSE frame contract, and the one OAuth entry URL on every client page.
#
# Usage:
#   scripts/ref-docs-check.sh          check: exit 0, or exit 1 naming each
#                                      drifted file.
#   scripts/ref-docs-check.sh --write  regenerate the generated sections.
set -uo pipefail
cd "$(dirname "$0")/.."

MODE="${1:-check}"
[ "$MODE" = "--write" ] && MODE="write"

CAP_START="<!-- capabilities:start -->"
CAP_END="<!-- capabilities:end -->"
CAP_TARGETS=("www/llms.txt" "docs/protocol/capabilities.md")

mcphost_bin() {
  local bin="${MCPHOST_BIN:-${CARGO_BIN_EXE_mcphost:-}}"
  if [ -n "$bin" ]; then
    "$bin" "$@"
    return $?
  fi
  local cand
  for cand in target/release/mcphost target/debug/mcphost; do
    if [ -x "$cand" ] && [ -z "$(find src Cargo.toml Cargo.lock -newer "$cand" -print -quit 2>/dev/null)" ]; then
      "$cand" "$@"
      return $?
    fi
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

write_capabilities() {
  local section doc
  section="$(mcphost_bin capabilities-section)" || return $?
  mkdir -p docs/protocol
  mcphost_bin capabilities-section --doc > docs/protocol/capabilities.md || return $?
  python3 - www/llms.txt "$CAP_START" "$CAP_END" "$section" <<'PY'
import sys

path, start, end, section = sys.argv[1:5]
with open(path, encoding="utf-8") as f:
    text = f.read()
if start in text and end in text:
    text = text[: text.index(start)] + section + text[text.index(end) + len(end) :]
elif "\n## Contract\n" in text:
    i = text.index("\n## Contract\n")
    text = text[:i] + "\n" + section + "\n" + text[i:]
else:
    text = text.rstrip("\n") + "\n\n" + section + "\n"
with open(path, "w", encoding="utf-8") as f:
    f.write(text)
PY
}

if [ "$MODE" = "write" ]; then
  write_capabilities || exit $?
  echo "ref-docs-check.sh: wrote capabilities"
  # PRD-mcphost-dry-run-on-write-verbs R5: docs/host-tools.md, generated
  # from src/write_verbs.rs.
  "$(dirname "$0")/host-tools-doc-check.sh" --write || exit $?
  # PRD-mcphost-sse-frame-contract-generated: frames.md/frames.json and the
  # llms.txt Transport block, generated from FRAME_SEQUENCES.
  "$(dirname "$0")/protocol-frames-check.sh" --write || exit $?
  exit 0
fi

FAILURES=0
CAP_EXPECTED="$(mcphost_bin capabilities-section)" || {
  echo "ref-docs-check.sh: could not render the capabilities section (mcphost capabilities-section failed)" >&2
  exit 2
}
for f in "${CAP_TARGETS[@]}"; do
  if [ ! -f "$f" ]; then
    echo "ref-docs-check.sh: $f does not exist (run ref-docs-check.sh --write)" >&2
    FAILURES=$((FAILURES + 1))
    continue
  fi
  CAP_ACTUAL="$(cap_block_of "$f")"
  if [ "$CAP_ACTUAL" != "$CAP_EXPECTED" ]; then
    echo "ref-docs-check.sh: $f ## Capabilities section differs from render_capabilities_section() (run ref-docs-check.sh --write)" >&2
    FAILURES=$((FAILURES + 1))
  fi
done

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
  echo "ref-docs-check.sh: could not read OAUTH_ENTRY_PATH from src/oauth.rs" >&2
  FAILURES=$((FAILURES + 1))
else
  for f in README.md llms-install.md www/llms-install.md www/llms.txt www/llms-full.txt www/index.html docs/oauth-connect.md; do
    if ! grep -qF "https://mcphost.dev${ENTRY_PATH}" "$f"; then
      echo "ref-docs-check.sh: $f does not name the OAuth entry URL https://mcphost.dev${ENTRY_PATH}" >&2
      FAILURES=$((FAILURES + 1))
    fi
    stray=$(grep -oE 'https://mcphost\.dev/login/[A-Za-z0-9_./-]*' "$f" | grep -vxF "https://mcphost.dev${ENTRY_PATH}" || true)
    if [ -n "$stray" ]; then
      echo "ref-docs-check.sh: $f names an entry path other than ${ENTRY_PATH}: $stray" >&2
      FAILURES=$((FAILURES + 1))
    fi
  done
fi

if [ "$FAILURES" -gt 0 ]; then
  echo "ref-docs-check.sh: FAILED (${FAILURES} issue(s))" >&2
  exit 1
fi

echo "ref_docs_in_sync=true"
exit 0
