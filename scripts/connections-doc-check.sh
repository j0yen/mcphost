#!/usr/bin/env bash
# connections-doc-check.sh -- PRD-mcphost-connection-provider-registry R5:
# the CI form of tests/cprov_ac02_provider_surfaces_cannot_drift.rs. Reads the
# provider names straight out of providers.toml (never a second hand-kept
# list) and diffs them against the two committed prose surfaces,
# docs/connections.md (its `## \`name\`` headings) and the
# `<!-- connections:start/end -->` section of www/llms.txt (its `- \`name\``
# bullets). Prints one `surface=<s> providers=<n> in_sync=<true|false>` line
# per surface and exits 1, naming every extra or missing provider on stderr,
# the moment either surface differs; exits 0 only when both agree.
#
# Usage:
#   scripts/connections-doc-check.sh [repo-root]
#     repo-root defaults to "." -- overridable so a test can point this at a
#     small fixture tree (<root>/providers.toml, <root>/docs/connections.md,
#     <root>/www/llms.txt).
set -uo pipefail
ROOT="${1:-.}"

python3 - "$ROOT" <<'PY'
import os
import re
import sys
import tomllib

root = sys.argv[1]


def read(rel):
    path = os.path.join(root, rel)
    try:
        with open(path, encoding="utf-8") as fh:
            return fh.read()
    except OSError as e:
        print(f"connections-doc-check: cannot read {rel}: {e}", file=sys.stderr)
        sys.exit(1)


expected = set(tomllib.loads(read("providers.toml")).keys())

doc_names = set(re.findall(r"^## `([^`]+)`$", read("docs/connections.md"), re.MULTILINE))

llms = read("www/llms.txt")
start = llms.find("<!-- connections:start -->")
end = llms.find("<!-- connections:end -->")
llms_names = set()
if start != -1 and end > start:
    llms_names = set(re.findall(r"^- `([^`]+)`", llms[start:end], re.MULTILINE))

status = 0
for surface, names in (("docs/connections.md", doc_names), ("www/llms.txt", llms_names)):
    extra = sorted(names - expected)
    missing = sorted(expected - names)
    print(f"surface={surface} providers={len(names)} in_sync={'true' if not extra and not missing else 'false'}")
    for n in extra:
        print(f"connections-doc-check: {surface} names '{n}', which is not in providers.toml", file=sys.stderr)
    for n in missing:
        print(f"connections-doc-check: {surface} is missing '{n}', which is in providers.toml", file=sys.stderr)
    if extra or missing:
        status = 1
sys.exit(status)
PY
