#!/usr/bin/env python3
"""timing-lint -- PRD-mcphost-timing-assertion-lint.

Fails any `assert!`/`assert_eq!`/`assert_ne!`/`debug_assert!` family
invocation under `tests/**/*.rs` (outside `tests/common/`) whose argument text
contains a wall-clock token, unless the invocation sits inside the body of a
sanctioned timing helper.

The sanctioned set is READ from `<target>/common/mod.rs`: every
`macro_rules! NAME` or `pub fn NAME` whose preceding doc comment carries the
line `/// timing-helper`. No helper name lives in this script.

Usage: timing-lint.py [TESTS_DIR]   (default: tests)
Output: one `file:line: <token>` per offender, then a summary line
`timing-lint: N offenders, M helpers`. Exit 1 on any offender, else 0.

Stdlib only. Brace matching is textual over source with comments and
string/char literals blanked out (same conservative posture as flake-lint.sh).
"""
import os
import re
import sys

MARKER = "/// timing-helper"
TOKEN_RE = re.compile(
    r"elapsed\s*\(|Duration::from_|as_millis\s*\(|as_micros\s*\(|"
    r"as_secs_f64\s*\(|as_secs"
)
ASSERT_RE = re.compile(r"\b(?:debug_)?assert(?:_eq|_ne)?\s*!\s*(?=[(\[{])")
CLOSERS = {"(": ")", "[": "]", "{": "}"}


def mask(src):
    """Blank comments and string/char literals, keeping length and newlines."""
    out = list(src)
    n = len(src)
    i = 0

    def blank(a, b):
        for k in range(a, min(b, n)):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        c = src[i]
        if src.startswith("//", i):
            j = src.find("\n", i)
            j = n if j < 0 else j
            blank(i, j)
            i = j
        elif src.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if src.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif src.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            blank(i, j)
            i = j
        elif c in "rb" and re.match(r"b?r#*\"", src[i:i + 40]) and (
            i == 0 or not (src[i - 1].isalnum() or src[i - 1] == "_")
        ):
            m = re.match(r"b?r(#*)\"", src[i:i + 40])
            close = '"' + m.group(1)
            j = src.find(close, i + m.end())
            j = n if j < 0 else j + len(close)
            blank(i, j)
            i = j
        elif c == '"':
            j = i + 1
            while j < n and src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            blank(i, j + 1)
            i = j + 1
        elif c == "'":
            if i + 1 < n and src[i + 1] == "\\":
                j = src.find("'", i + 3)
                j = n if j < 0 else j + 1
                blank(i, j)
                i = j
            elif i + 2 < n and src[i + 2] == "'":
                blank(i, i + 3)
                i += 3
            else:
                i += 1  # lifetime
        else:
            i += 1
    return "".join(out)


def match_close(masked, open_idx):
    """Index just past the delimiter matching masked[open_idx]."""
    stack = []
    for k in range(open_idx, len(masked)):
        ch = masked[k]
        if ch in CLOSERS:
            stack.append(CLOSERS[ch])
        elif ch in ")]}":
            if not stack or stack.pop() != ch:
                return len(masked)
            if not stack:
                return k + 1
    return len(masked)


def helper_names(common_mod):
    """Names of macros/fns under a `/// timing-helper` doc line."""
    try:
        lines = open(common_mod, encoding="utf-8").read().splitlines()
    except OSError:
        return []
    names = []
    for idx, line in enumerate(lines):
        m = re.match(r"\s*(?:macro_rules!\s+|pub\s+fn\s+)(\w+)", line)
        if not m:
            continue
        j = idx - 1
        while j >= 0 and re.match(r"\s*(///|#\[)", lines[j]):
            if lines[j].strip() == MARKER:
                names.append(m.group(1))
                break
            j -= 1
    return names


def scan(path, helpers):
    src = open(path, encoding="utf-8", errors="replace").read()
    masked = mask(src)
    regions = []
    for name in helpers:
        for m in re.finditer(r"\b%s\s*!?\s*(?=[(\[{])" % re.escape(name), masked):
            regions.append((m.start(), match_close(masked, m.end())))
    hits = []
    for m in ASSERT_RE.finditer(masked):
        if any(a <= m.start() < b for a, b in regions):
            continue
        end = match_close(masked, m.end())
        tok = TOKEN_RE.search(masked, m.end(), end)
        if tok:
            line = masked.count("\n", 0, m.start()) + 1
            hits.append((line, re.sub(r"\s+", "", tok.group(0))))
    return hits


def main(argv):
    root = argv[1] if len(argv) > 1 else "tests"
    helpers = helper_names(os.path.join(root, "common", "mod.rs"))
    offenders = 0
    for dirpath, dirnames, files in os.walk(root):
        dirnames[:] = sorted(d for d in dirnames if d != "common")
        for f in sorted(files):
            if not f.endswith(".rs"):
                continue
            path = os.path.join(dirpath, f)
            for line, tok in scan(path, helpers):
                offenders += 1
                print("%s:%d: %s" % (path, line, tok))
    print("timing-lint: %d offenders, %d helpers" % (offenders, len(helpers)))
    return 1 if offenders else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
