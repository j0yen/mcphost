#!/usr/bin/env bash
# tool-naming-lint.sh — PRD-mcphost-tool-naming-convention-and-aliases
# requirement 1 (AC1): enforces `host.<family>.<verb>` (docs/tool-naming.md)
# over every `Tool::new("...")` literal this crate registers.
#
# A name passes iff it is one of:
#   - a registered alias (src/tool_aliases.rs's own TOOL_ALIASES table --
#     grandfathered by construction, that's what an alias is);
#   - a documented top-level exception (signup, billing.*) or singleton
#     noun (no sibling verb under that noun) -- both listed, with their
#     reason, in EXCEPTIONS below;
#   - `host.<family>.<verb>` (at least one dot after `host.`).
# Anything else under `host.` is a violation, named alongside the
# canonical form it should have used instead (first `_` -> `.`, when
# there is one to split on).
#
# `admin.*` is out of scope entirely (PRD non-goal: "the admin.* surface
# ... separate rule") and never scanned.
#
# Usage:
#   scripts/tool-naming-lint.sh [root]
#     root defaults to "." -- overridable so a test can point this at a
#     small fixture tree (`<root>/src/**/*.rs`) without touching the real
#     repo, the same convention scripts/spec-fields-doc-check.sh's own
#     `[repo-root]` argument uses.
set -uo pipefail
ROOT="${1:-.}"

python3 - "$ROOT" <<'PY'
import glob
import os
import re
import sys

root = sys.argv[1]

# Requirement 3's own "singleton noun" clause: a host.<noun> tool with no
# sibling verb under that noun. Reasons are printed verbatim by this
# script -- docs/tool-naming.md's own Exceptions section quotes the same
# list, but THIS is the authoritative, machine-checked copy.
EXCEPTIONS = [
    ("signup", "predates the host. namespace; the first word every client learns"),
    ("billing.*", "predates the host. namespace; its own top-level family, not host.*"),
    ("host.whoami", "singleton noun: no sibling verb under 'whoami'"),
    ("host.redeem", "singleton noun: no sibling verb under 'redeem'"),
    ("host.quickstart", "singleton noun: no sibling verb under 'quickstart'"),
    ("host.usage", "singleton noun: no sibling verb under 'usage'"),
    ("host.changelog", "singleton noun: no sibling verb under 'changelog'"),
    ("host.export", "singleton noun: no sibling verb under 'export'"),
    ("host.progress", "singleton noun: no sibling verb under 'progress'"),
]
EXCEPTION_NAMES = {name for name, _ in EXCEPTIONS}

TOOL_NEW_RE = re.compile(r'Tool::new\(\s*"([^"]+)"', re.MULTILINE)
ALIAS_ROW_RE = re.compile(r'alias:\s*"([^"]+)"\s*,\s*canonical:\s*"([^"]+)"')


def rust_files(base):
    pattern = os.path.join(base, "**", "*.rs")
    return sorted(glob.glob(pattern, recursive=True))


def extract_tool_names(base):
    names = []
    for path in rust_files(os.path.join(base, "src")):
        with open(path, encoding="utf-8") as fh:
            text = fh.read()
        names.extend((n, path) for n in TOOL_NEW_RE.findall(text))
    return names


def extract_aliases(base):
    path = os.path.join(base, "src", "tool_aliases.rs")
    if not os.path.isfile(path):
        return []
    with open(path, encoding="utf-8") as fh:
        text = fh.read()
    return ALIAS_ROW_RE.findall(text)


def canonical_suggestion(local):
    if "_" in local:
        return "host." + local.replace("_", ".", 1)
    return None


def main():
    tool_names = extract_tool_names(root)
    aliases = extract_aliases(root)
    alias_names = {alias for alias, _ in aliases}

    for name, reason in EXCEPTIONS:
        print(f'exception={name} reason="{reason}"')

    violations = []
    for name, path in tool_names:
        if name in alias_names or name in EXCEPTION_NAMES:
            continue
        if not name.startswith("host.") or name.startswith("billing."):
            # out of scope: admin.*, a tenant-dynamic name, or billing.*
            # itself (a documented exception matched by prefix, not by
            # exact name, since billing.plans/status/checkout never
            # registers "billing.*" literally).
            continue
        local = name[len("host."):]
        if "." in local:
            continue  # host.<family>.<verb> -- compliant
        suggestion = canonical_suggestion(local)
        violations.append((name, suggestion, path))

    for name, suggestion, path in violations:
        if suggestion:
            print(
                f"tool-naming-lint: {name} violates rule (use {suggestion}) [{path}]",
                file=sys.stderr,
            )
        else:
            print(
                f"tool-naming-lint: {name} violates rule (no family/verb to split; "
                "name it host.<family>.<verb> explicitly) [{path}]".format(path=path),
                file=sys.stderr,
            )

    print(f"violations={len(violations)} aliases={len(aliases)}")
    sys.exit(1 if violations else 0)


main()
PY
