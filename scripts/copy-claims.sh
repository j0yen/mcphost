#!/usr/bin/env bash
# copy-claims.sh — PRD-mcphost-agent-findability P0#4 ("claim audit"):
# no numeric performance claim or marketing superlative in the public copy
# is allowed to stand unbacked. Scans README.md, www/index.html,
# www/llms.txt, and server.json's description for:
#
#   - numeric performance/timing claims: "p95 ...<number>", "median ...
#     <number> (ms|s|seconds|minutes)", "under N minutes/seconds", or a
#     "measured ... <number>" sentence
#   - marketing superlatives: "(the) only|fastest|best <runtime|host|MCP
#     host|platform|solution|way>"
#
# and requires an `<!-- cite: <path> -->` HTML-comment citation within a
# few lines of the claim. For a numeric claim, the cited path must exist
# in this repo AND its content must literally contain the claimed number
# (a real benchmark/measure-run mirror, not just a name-drop) -- see
# docs/benchmarks/*.md and docs/agent-quickstart.md for the convention.
#
# Deliberately narrow on "only": a blanket bare-word match on "only" would
# flag correct, already-tested API statements ("the only tool offered is
# signup", "key stored only as a hash", "present only when ...") that are
# structural facts about the protocol, not comparative marketing claims,
# and most already sit next to a test-file citation in the same README
# table row. The PRD's own examples of unbacked claims ("under five
# minutes", "the only runtime") are both comparative/performance phrasing,
# which the narrower regexes below catch.
set -uo pipefail
cd "$(dirname "$0")/.."

FILES=(README.md www/index.html www/llms.txt server.json)
status=0

for f in "${FILES[@]}"; do
  [ -f "$f" ] || continue
  python3 - "$f" <<'PY' || status=1
import re
import sys

path = sys.argv[1]
with open(path, encoding="utf-8") as fh:
    lines = fh.readlines()
text = "".join(lines)

CITE_RE = re.compile(r"<!--\s*cite:\s*(\S+?)\s*-->")

NUMERIC_PATTERNS = [
    re.compile(r"\bp9[0-9]\b\s*(=|:|of)?\s*[0-9]", re.IGNORECASE),
    re.compile(r"\bmedian\b.{0,60}?[0-9]+(\.[0-9]+)?\s?(ms|milliseconds?|s\b|seconds?|minutes?)", re.IGNORECASE),
    re.compile(r"\bunder\s+[a-z0-9]+\s+(minutes?|seconds?)\b", re.IGNORECASE),
    re.compile(r"\bmeasured\b.{0,80}?[0-9]", re.IGNORECASE),
]
SUPERLATIVE_PATTERN = re.compile(
    r"\b(the\s+)?(only|fastest|best)\s+(runtime|host|mcp\s+host|platform|solution|way)\b",
    re.IGNORECASE,
)
NUMBER_RE = re.compile(r"[0-9]+(?:\.[0-9]+)?")

failures = []

for lineno, line in enumerate(lines, start=1):
    claims = []
    for pat in NUMERIC_PATTERNS:
        for m in pat.finditer(line):
            claims.append(("numeric", m.group(0)))
    for m in SUPERLATIVE_PATTERN.finditer(line):
        claims.append(("superlative", m.group(0)))
    if not claims:
        continue

    # citation window: this line plus up to 3 lines before/after
    window_start = max(0, lineno - 4)
    window_end = min(len(lines), lineno + 6)
    window_text = "".join(lines[window_start:window_end])
    cites = CITE_RE.findall(window_text)

    if not cites:
        for kind, snippet in claims:
            failures.append(f"{path}:{lineno}: {kind} claim {snippet!r} has no <!-- cite: ... --> nearby")
        continue

    for kind, snippet in claims:
        if kind == "superlative":
            # any citation in the window is sufficient for a superlative
            continue
        # numeric: at least one cited file must exist and contain the number
        numbers = NUMBER_RE.findall(snippet)
        if not numbers:
            continue
        ok = False
        reasons = []
        for cite in cites:
            cite_path = cite
            import os
            if not os.path.isfile(cite_path):
                reasons.append(f"cited path {cite_path!r} does not exist")
                continue
            with open(cite_path, encoding="utf-8", errors="replace") as cf:
                cited_text = cf.read()
            if any(n in cited_text for n in numbers):
                ok = True
                break
            reasons.append(f"{cite_path!r} does not contain {numbers}")
        if not ok:
            failures.append(
                f"{path}:{lineno}: numeric claim {snippet!r} cited {cites} but " + "; ".join(reasons)
            )

if failures:
    for f in failures:
        print(f, file=sys.stderr)
    sys.exit(1)
PY
done

exit "$status"
