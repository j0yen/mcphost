#!/usr/bin/env bash
# connections-doc-check.sh -- PRD-mcphost-connection-provider-registry R5:
# the provider names in docs/connections.md and in the `## Upstream
# connections` section of www/llms.txt must equal the non-test_only rows of
# providers.toml (read straight from that file, never a second list). Exits 1
# naming the surface and the extra (or missing) provider, 0 when in sync.
#
# Usage:
#   scripts/connections-doc-check.sh [repo-root]
#     repo-root defaults to "." -- overridable so a test can point it at a
#     fixture tree.
set -uo pipefail
ROOT="${1:-.}"

python3 - "$ROOT" <<'PY'
import os
import re
import sys
import tomllib

root = sys.argv[1]
with open(os.path.join(root, "providers.toml"), "rb") as f:
    table = tomllib.load(f)
expected = sorted(n for n, row in table.items() if not row.get("test_only", False))

def read(rel):
    with open(os.path.join(root, rel), encoding="utf-8") as f:
        return f.read()

doc = read("docs/connections.md")
doc_names = re.findall(r"^## `([^`]+)`$", doc, re.M)

llms = read("www/llms.txt")
start, end = llms.find("<!-- connections:start -->"), llms.find("<!-- connections:end -->")
llms_names = re.findall(r"^- `([^`]+)` \(", llms[start:end], re.M) if 0 <= start < end else []

bad = []
for surface, names in (("docs/connections.md", doc_names), ("www/llms.txt", llms_names)):
    for n in names:
        if n not in expected:
            bad.append(f"{surface}: names provider '{n}' that is not in providers.toml")
    for n in expected:
        if n not in names:
            bad.append(f"{surface}: is missing provider '{n}' from providers.toml")
for line in bad:
    print(line, file=sys.stderr)
print(f"providers={len(expected)} in_sync={'false' if bad else 'true'}")
sys.exit(1 if bad else 0)
PY
