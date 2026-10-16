#!/usr/bin/env bash
# contract-lint.sh -- PRD-mcphost-contract-suite-generated, derived by
# PRD-mcphost-contract-suite-markers.
#
# A test that asserts a set the repository derives from itself is a CONTRACT
# test: it declares `//! contract: <what it asserts>` in its first 10 lines so
# scripts/gen-test-suites.sh routes it into a tests/suite_contracts_<core|sandbox>_NN.rs
# binary that scripts/pin-tests.sh (the inner's `contract-suite` check and CI's
# `contract suites` step) runs before the full suite.
#
# Nothing here is a hand list. The artifact set A is DERIVED from the marked
# tests: every repo path outside tests/ that a marked test names next to a
# CARGO_MANIFEST_DIR join. An unmarked test that reads a path in A is flagged
# with the path and one marked test that defines it.
#
# Usage:
#   scripts/contract-lint.sh [DIR-or-FILE]   defaults to `tests`
#   scripts/contract-lint.sh --fix [DIR]     print `<file>: add  //! contract: reads <artifact>` per flagged file; edits nothing
#   scripts/contract-lint.sh --artifacts     print the derived artifact set, one repo path per line
# Exit 0 clean, 1 on any unmarked contract-shaped file.
set -uo pipefail
cd "$(dirname "$0")/.."

python3 -I - "$@" <<'PY'
import os
import re
import sys

FLAGS = {a for a in sys.argv[1:] if a.startswith("--")}
POS = [a for a in sys.argv[1:] if not a.startswith("--")]
TARGET = POS[0] if POS else "tests"

REPO = os.getcwd()
HELPERS = ("suite_core_", "suite_sandbox_", "suite_env_", "suite_contracts_")


def list_files(target):
    if os.path.isdir(target):
        out = []
        for d, _, fs in os.walk(target):
            out += [os.path.join(d, f) for f in fs if f.endswith(".rs")]
        return sorted(out)
    return [target]


def is_helper(path, target):
    # Only top-level files of the target are tests; subdirectories hold helpers.
    if os.path.isdir(target) and os.sep in os.path.relpath(path, target):
        return True
    return os.path.basename(path).startswith(HELPERS)


def marker(text):
    for l in text.splitlines()[:10]:
        if l.startswith("//! contract:"):
            return l[len("//! contract:"):].strip()
    return None


def is_marked(text):
    return marker(text) is not None


def defines_artifacts(text):
    # A `contract: reads <artifact>` or `walks <dir>/` line is the follower marker this lint asks
    # authors to add; it routes the file into the family but defines nothing,
    # so marking followers never widens the artifact set (no cascade).
    m = marker(text)
    return m is not None and not m.startswith(("reads ", "walks "))


JOIN = re.compile(r'\.join\(\s*"([^"]*)"\s*\)')
LIT = re.compile(r'"([^"\n]*)"')


def candidates(text):
    """String literals (and consecutive .join("..") chains) naming repo paths."""
    out = set()
    for lit in LIT.findall(text):
        out.add(lit)
    for m in re.finditer(r'(?:\.join\(\s*"[^"]*"\s*\)\s*){2,}', text):
        parts = JOIN.findall(m.group(0))
        out.add("/".join(p.strip("/") for p in parts))
    return out


def roots():
    r = [REPO]
    if os.path.isdir(TARGET):
        r.append(os.path.dirname(os.path.abspath(TARGET)))
    return r


def reads(text):
    """Repo files outside tests/ that this file names beside CARGO_MANIFEST_DIR."""
    cands = set()
    # include_str!("../<path>") is a tree read with no CARGO_MANIFEST_DIR join.
    for m in re.finditer(r'include_(?:str|bytes)!\(\s*"\.\./([^"]+)"', text):
        cands.add(m.group(1))
    if "CARGO_MANIFEST_DIR" in text:
        cands |= candidates(text)
    found = set()
    for c in cands:
        c = c.lstrip("./") if c.startswith("./") else c
        if not c or c.startswith(("tests/", "/")) or ".." in c.split("/") or " " in c:
            continue
        for root in roots():
            if os.path.isfile(os.path.join(root, c)):
                found.add(c)
                break
    return found


def load(target):
    out = []
    for f in list_files(target):
        if not os.path.isfile(f) or is_helper(f, target):
            continue
        with open(f, encoding="utf-8", errors="replace") as fh:
            text = fh.read()
        out.append((f, text, is_marked(text)))
    return out


def marker_paths(text):
    """Repo files the marker line itself names (a test that reads an artifact
    through a library constant, not a path, says so in its marker)."""
    out = set()
    for tok in re.findall(r"[A-Za-z0-9_./-]+", marker(text) or ""):
        tok = tok.strip(".")
        if tok and not tok.startswith("tests/") and any(
            os.path.isfile(os.path.join(r, tok)) for r in roots()
        ):
            out.add(tok)
    return out


files = load(TARGET)
# The committed tests/ always define artifacts, so a fixture tree is judged
# against the same derived set as the real one.
definers = [(f, t) for f, t, m in files if m and defines_artifacts(t)]
if os.path.isdir("tests") and os.path.abspath(TARGET) != os.path.abspath("tests"):
    definers += [(f, t) for f, t, m in load("tests") if m and defines_artifacts(t)]

defines = {}
for f, t in definers:
    for a in sorted(reads(t) | marker_paths(t)):
        defines.setdefault(a, f)

def walked_dirs(text):
    """Repo directories outside tests/ that this file walks with read_dir(."""
    if "CARGO_MANIFEST_DIR" not in text or "read_dir(" not in text:
        return set()
    # Only literals in the same statement as a CARGO_MANIFEST_DIR join count.
    found = set()
    for stmt in re.split(r"[;{}]", text):
        if "CARGO_MANIFEST_DIR" not in stmt:
            continue
        for c in candidates(stmt):
            c = c.strip("/")
            if not c or c.startswith(("tests", "target")) or ".." in c.split("/") or " " in c:
                continue
            if any(os.path.isdir(os.path.join(r, c)) for r in roots()):
                found.add(c)
    return found


if "--artifacts" in FLAGS:
    for a in sorted(defines):
        print(a)
    sys.exit(0)

FIX = "--fix" in FLAGS
bad = 0
for f, t, m in files:
    if m:
        continue
    walked = sorted(walked_dirs(t))
    if walked and FIX:
        print(f"{f}: add  //! contract: walks {walked[0]}/")
        bad = 1
        continue
    if walked:
        print(f"contract-lint: {f} walks {walked[0]}/ with read_dir( (tree-walk rule) -- add the line `//! contract: <what it asserts>` to its first 10 lines")
        bad = 1
        continue
    for a in sorted(reads(t) & set(defines)):
        if FIX:
            print(f"{f}: add  //! contract: reads {a}")
            bad = 1
            break
        print(f"contract-lint: {f} reads {a} (defined by marked {defines[a]}) -- add the line `//! contract: reads {a}` to its first 10 lines")
        bad = 1
        break

if bad and FIX:
    sys.exit(1)
if bad:
    print("contract-lint: FAILED")
    sys.exit(1)
nmarked = sum(1 for _, _, m in files if m)
print(f"contract-lint: ok ({nmarked} marked files)")
PY
