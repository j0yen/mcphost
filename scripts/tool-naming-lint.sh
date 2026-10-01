#!/usr/bin/env bash
# tool-naming-lint.sh -- PRD-mcphost-tool-naming-convention-and-aliases
# requirement 1 (AC1): the naming rule (host.<family>.<verb>) encoded as a
# regex over `Tool::new("…")` literals, checked against the single source
# of truth in src/tool_aliases.rs (ALIASES, SINGLETON_NOUN_EXCEPTIONS) so
# this script and the registry/dispatch/tools-list code can never drift
# about which names are grandfathered aliases vs. genuine new violations.
#
# Exceptions (never a violation):
#   - admin.*            operator-only surface, a separate naming rule
#                         entirely (PRD non-goals) -- out of scope.
#   - signup, billing.*  documented top-level exceptions (PRD TL;DR):
#                         they predate the host. namespace.
#   - SINGLETON_NOUN_EXCEPTIONS (src/tool_aliases.rs) -- a tool that is
#                         itself one word with no sibling verb keeps
#                         host.<noun> rather than a trivial
#                         host.<noun>.<noun> split.
#   - ALIASES' alias side (src/tool_aliases.rs) -- a grandfathered legacy
#                         name this PRD itself gave a canonical+alias to;
#                         not a NEW violation.
#
# Usage:
#   scripts/tool-naming-lint.sh [registry-file]   default: src/handler.rs
#
# Exit 0: prints each exception with its reason, then
#   "tool-naming-lint: violations=0 aliases=<n>".
# Exit 1: one line per violation, each naming the offending name and (when
#   mechanically derivable -- the "noun_verb" shape every real violator in
#   this repo has had so far) a suggested canonical replacement.
set -euo pipefail
cd "$(dirname "$0")/.."

REGISTRY="${1:-src/handler.rs}"
ALIASES_SRC="src/tool_aliases.rs"

if [ ! -f "$REGISTRY" ]; then
  echo "tool-naming-lint: no such registry file: $REGISTRY" >&2
  exit 2
fi
if [ ! -f "$ALIASES_SRC" ]; then
  echo "tool-naming-lint: no such aliases source: $ALIASES_SRC" >&2
  exit 2
fi

python3 - "$REGISTRY" "$ALIASES_SRC" <<'PY'
import re
import sys

registry_path, aliases_path = sys.argv[1], sys.argv[2]
registry = open(registry_path, encoding="utf-8").read()
aliases_src = open(aliases_path, encoding="utf-8").read()

# Single source of truth: parse ALIASES and SINGLETON_NOUN_EXCEPTIONS
# straight out of src/tool_aliases.rs rather than hand-mirroring them here.
alias_pairs = re.findall(r'\("([a-zA-Z0-9_.]+)",\s*"([a-zA-Z0-9_.]+)"\)', aliases_src)
aliases = {alias: canonical for alias, canonical in alias_pairs}

singleton_block_match = re.search(
    r"SINGLETON_NOUN_EXCEPTIONS:\s*&\[&str\]\s*=\s*&\[(.*?)\];", aliases_src, re.DOTALL
)
assert singleton_block_match, "could not find SINGLETON_NOUN_EXCEPTIONS in " + aliases_path
singletons = set(re.findall(r'"([a-zA-Z0-9_.]+)"', singleton_block_match.group(1)))

TOP_LEVEL_EXCEPTIONS = {"signup"}
EXCEPTION_FAMILY_PREFIXES = ("billing.",)
OUT_OF_SCOPE_PREFIX = "admin."

# Every `Tool::new("name", ...)` literal (dynamically-built names, e.g. the
# per-tenant shared-tool descriptor's `format!("{}.{}", ...)`, are not
# string literals and so never match -- out of scope for a static lint by
# construction, same as the audit's own "excluded from this count" note).
names = sorted(set(re.findall(r'Tool::new\(\s*"([a-zA-Z0-9_.]+)"', registry)))

def suggest(name: str) -> str | None:
    rest = name[len("host."):]
    if "." in rest or "_" not in rest:
        return None
    family, verb = rest.split("_", 1)
    return f"host.{family}.{verb}"

violations: list[tuple[str, str, str | None]] = []
exceptions: list[tuple[str, str]] = []

for name in names:
    if name.startswith(OUT_OF_SCOPE_PREFIX):
        continue  # admin.* -- separate rule, out of scope entirely
    if name in TOP_LEVEL_EXCEPTIONS:
        exceptions.append((name, "documented top-level exception (predates the host. namespace)"))
        continue
    if any(name.startswith(p) for p in EXCEPTION_FAMILY_PREFIXES):
        exceptions.append((name, "documented top-level family exception"))
        continue
    if name in singletons:
        exceptions.append((name, "singleton noun -- no sibling verb under this name"))
        continue
    if name in aliases:
        exceptions.append((name, f"grandfathered alias of {aliases[name]}"))
        continue
    if not name.startswith("host."):
        violations.append((name, "must start with host. (or be a documented exception)", None))
        continue
    rest = name[len("host."):]
    parts = rest.split(".")
    if len(parts) != 2:
        violations.append(
            (name, f"host.<family>.<verb> expected, got {len(parts)} segment(s) after host.", suggest(name))
        )
        continue
    family, _verb = parts
    if "_" in family:
        violations.append(
            (name, f"family segment '{family}' must not contain '_'", f"host.{family.replace('_', '.')}.{_verb}")
        )
        continue

if violations:
    for name, reason, suggestion in violations:
        hint = f"; use {suggestion}" if suggestion else "; add it to host.<family>.<verb> or to SINGLETON_NOUN_EXCEPTIONS with a reason"
        print(f"tool-naming-lint: {name} violates the naming rule ({reason}){hint}", file=sys.stderr)
    sys.exit(1)

for name, reason in exceptions:
    print(f"tool-naming-lint: exception {name} ({reason})")
print(f"tool-naming-lint: violations=0 aliases={len(aliases)}")
PY
