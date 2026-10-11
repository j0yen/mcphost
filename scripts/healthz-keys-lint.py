#!/usr/bin/env python3
"""healthz-keys-lint.py -- PRD-mcphost-healthz-anon-keys-const.

Fails any tests/**/*.rs (outside tests/common/) that spells the anonymous
/healthz key set as a literal: an array or `json!` object naming every key of
`ANON_HEALTHZ_KEYS` within a window of at most 6 lines. Tests must read
`common::anon_healthz_keys()` instead.

The key names are parsed from `ANON_HEALTHZ_KEYS` in `<root>/src/build_info.rs`
(root = this script's parent's parent), never spelled here, so the detection
set changes with the const.

Usage: scripts/healthz-keys-lint.py [tests-dir]   (default: tests)
Exit 0 clean, 1 findings (`file:line: ...`), 2 usage/parse error.
"""
import re
import sys
from pathlib import Path

WINDOW = 6
ROOT = Path(__file__).resolve().parent.parent


def anon_keys():
    src = (ROOT / "src" / "build_info.rs").read_text(encoding="utf-8")
    m = re.search(r"ANON_HEALTHZ_KEYS\s*:\s*&\[&str\]\s*=\s*&\[(.*?)\]\s*;", src, re.S)
    if not m:
        sys.exit("healthz-keys-lint: ANON_HEALTHZ_KEYS not found in src/build_info.rs")
    keys = re.findall(r'"([^"]+)"', m.group(1))
    if not keys:
        sys.exit("healthz-keys-lint: ANON_HEALTHZ_KEYS is empty")
    return keys


# `x["k"]` / `.get("k")` are value reads, not a key-set literal.
INDEX_OR_CALL = re.compile(r'[\[(]\s*"[^"]*"\s*[\])]')


def names_on(line, keys):
    line = INDEX_OR_CALL.sub("", line)
    return {k for k in keys if f'"{k}"' in line}


def scan(path, keys):
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    named = [names_on(ln, keys) for ln in lines]
    found, i = [], 0
    while i < len(lines):
        if named[i]:
            seen = set()
            for j in range(i, min(i + WINDOW, len(lines))):
                seen |= named[j]
                if len(seen) == len(keys):
                    found.append(i + 1)
                    i = j
                    break
        i += 1
    return found


def main():
    tests = Path(sys.argv[1] if len(sys.argv) > 1 else "tests")
    if not tests.is_dir():
        print(f"healthz-keys-lint: not a directory: {tests}", file=sys.stderr)
        return 2
    keys = anon_keys()
    bad = 0
    for path in sorted(tests.rglob("*.rs")):
        if "common" in path.relative_to(tests).parts[:-1]:
            continue
        for ln in scan(path, keys):
            bad += 1
            print(f"{path}:{ln}: anonymous /healthz key set spelled as a literal; use common::anon_healthz_keys()")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
