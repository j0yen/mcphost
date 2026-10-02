#!/usr/bin/env bash
# spec-fields-doc-check.sh — PRD-mcphost-spec-unknown-field-rejection
# requirement 6 (AC7): diffs each kind's known-field list -- read straight
# out of `src/kinds/<kind>.rs`'s own `known_spec_fields()` literal, never a
# second hand-maintained copy -- against its `docs/kinds/<kind>.md` page.
# Prints one `kind=<k> fields=<n> doc_in_sync=<true|false>` line per kind
# (sorted by kind name) and exits 1 (naming every offending field, one per
# line, on stderr) the moment any kind is out of sync; exits 0 only when
# every kind is in sync.
#
# "In sync" is two one-directional checks, same split requirement 6's own
# "lists a field the parser lacks, or vice versa" names:
#   1. every top-level key in the doc page's FIRST ```json example (the
#      same block `kinds::docs::parse_kind_doc` treats as the canonical
#      spec example) must be a field the parser actually reads -- a doc
#      page cannot advertise a field that silently no-ops (this PRD's own
#      Problem statement).
#   2. every field the parser reads must appear somewhere in the page's
#      own text -- a field the parser added and the docs never mention.
#
# Usage:
#   scripts/spec-fields-doc-check.sh [repo-root]
#     repo-root defaults to "." -- overridable so a test can point this at
#     a small fixture tree (`<root>/src/kinds/<kind>.rs` +
#     `<root>/docs/kinds/<kind>.md`) without touching the real repo.
set -uo pipefail
ROOT="${1:-.}"

python3 - "$ROOT" <<'PY'
import glob
import os
import re
import sys

root = sys.argv[1]
docs_dir = os.path.join(root, "docs", "kinds")
kinds_dir = os.path.join(root, "src", "kinds")

doc_paths = sorted(glob.glob(os.path.join(docs_dir, "*.md")))
if not doc_paths:
    print(f"spec-fields-doc-check: no docs/kinds/*.md under {root}", file=sys.stderr)
    sys.exit(1)

FIELD_RE = re.compile(r'"([a-zA-Z_][a-zA-Z0-9_]*)"')
FN_RE = re.compile(
    r"fn\s+known_spec_fields\s*\([^)]*\)[^{]*\{\s*&\[(.*?)\]\s*\}",
    re.DOTALL,
)


def known_fields(kind: str) -> list[str]:
    """The exact list `Kind::known_spec_fields()` returns for `kind`,
    parsed straight out of its own source file -- see this script's own
    header doc for why a second hand-maintained list would defeat the
    point."""
    src_path = os.path.join(kinds_dir, f"{kind}.rs")
    with open(src_path, encoding="utf-8") as fh:
        text = fh.read()
    m = FN_RE.search(text)
    if not m:
        print(f"spec-fields-doc-check: no known_spec_fields() found in {src_path}", file=sys.stderr)
        sys.exit(1)
    return FIELD_RE.findall(m.group(1))


def first_json_block_keys(doc_text: str) -> list[str]:
    """Top-level keys of the first ```json fenced block in `doc_text` --
    the same block `kinds::docs::parse_kind_doc` treats as the canonical
    spec example. A plain brace/quote scan, not a JSON object-nesting
    parser: good enough for the flat top-level spec objects every
    docs/kinds/*.md page shows, and this script never needs to look past
    the top level."""
    m = re.search(r"```json\s*\n(.*?)```", doc_text, re.DOTALL)
    if not m:
        return []
    block = m.group(1)
    # Only the block's OWN top-level keys: a key line is "<indent>"name": ,
    # indented exactly one level (2 or 4 spaces) under the opening `{` --
    # a nested object's own keys sit one level deeper and are excluded by
    # requiring the line's leading whitespace to be the shallowest seen.
    lines = block.splitlines()
    key_lines = [(len(l) - len(l.lstrip(" ")), l) for l in lines if re.match(r'^\s*"[a-zA-Z_]', l)]
    if not key_lines:
        return []
    min_indent = min(indent for indent, _ in key_lines)
    keys = []
    for indent, line in key_lines:
        if indent != min_indent:
            continue
        km = re.match(r'^\s*"([a-zA-Z_][a-zA-Z0-9_]*)"\s*:', line)
        if km:
            keys.append(km.group(1))
    return keys


status = 0
for doc_path in doc_paths:
    kind = os.path.splitext(os.path.basename(doc_path))[0]
    with open(doc_path, encoding="utf-8") as fh:
        doc_text = fh.read()

    fields = known_fields(kind)
    problems = []

    doc_keys = first_json_block_keys(doc_text)
    for key in doc_keys:
        if key not in fields:
            problems.append(f"docs/kinds/{kind}.md's example spec names '{key}', which {kind}'s parser does not read")

    for field in fields:
        if field not in doc_text:
            problems.append(f"{kind}'s parser reads '{field}', which docs/kinds/{kind}.md never mentions")

    in_sync = not problems
    print(f"kind={kind} fields={len(fields)} doc_in_sync={'true' if in_sync else 'false'}")
    if problems:
        status = 1
        for p in problems:
            print(f"spec-fields-doc-check: {p}", file=sys.stderr)

sys.exit(status)
PY
