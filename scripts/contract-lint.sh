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
# Second derived axis (PRD-mcphost-contract-suite-registry-axis): a marked test
# NAMES the in-crate registry it walks, `//! contract: registry <mod::Symbol>[, ...] -- <what>`.
# The lint resolves each name to a `pub fn|const|static` under src/ and flags every
# unmarked test that calls (fn) or names (const|static) a registry symbol; the
# follower line is `//! contract: reads registry <mod::Symbol>`.
#
# Usage:
#   scripts/contract-lint.sh [DIR-or-FILE]   defaults to `tests`
#   scripts/contract-lint.sh --fix [DIR]     print `<file>: add  //! contract: reads <artifact | registry sym>` per flagged file; edits nothing
#   scripts/contract-lint.sh --definers      every artifact path and registry with its definer file and what that test asserts (TAB-separated)
#   scripts/contract-lint.sh --artifacts     print the derived artifact set, one repo path per line
#   scripts/contract-lint.sh --registries    print the derived registry set, one `registry <mod::Symbol>` per line
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
HELPERS = ("suite_core_", "suite_sandbox_", "suite_env_", "suite_contracts_", "suite_cargo_")


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


REGISTRY = "registry "
FOLLOW_REGISTRY = "reads registry "


def registry_names(text):
    """(is_definer, [mod::Symbol, ...]) from a `registry` / `reads registry` marker."""
    m = marker(text) or ""
    if m.startswith(FOLLOW_REGISTRY):
        definer, rest = False, m[len(FOLLOW_REGISTRY):]
    elif m.startswith(REGISTRY):
        definer, rest = True, m[len(REGISTRY):]
    else:
        return False, []
    rest = rest.split("\u2014")[0]
    return definer, [n.strip() for n in rest.split(",") if n.strip()]


def src_roots():
    r = []
    if os.path.isdir(TARGET):
        r += [os.path.dirname(os.path.abspath(TARGET)), os.path.abspath(TARGET)]
    return r + [REPO]


def resolve(sym):
    """'fn' | 'value' when src/ holds a matching pub item for mod::path::Symbol, else None."""
    if not re.fullmatch(r"[A-Za-z_]\w*(::[A-Za-z_]\w*)*", sym):
        return None
    *mods, name = sym.split("::")
    rels = [os.path.join("src", *mods) + ".rs", os.path.join("src", *mods, "mod.rs")] if mods else [os.path.join("src", "lib.rs")]
    pat = re.compile(r"^\s*pub(?:\([^)]*\))?\s+(fn|const|static)\s+" + re.escape(name) + r"\b", re.M)
    for root in src_roots():
        for rel in rels:
            path = os.path.join(root, rel)
            if os.path.isfile(path):
                with open(path, encoding="utf-8", errors="replace") as fh:
                    m = pat.search(fh.read())
                if m:
                    return "fn" if m.group(1) == "fn" else "value"
    return None


def code_only(text):
    return "\n".join(l for l in text.splitlines() if not l.lstrip().startswith("//"))


def names_it(text, sym, kind):
    """A definer's non-comment code must itself name the symbol it declares."""
    return walks_registry(code_only(text), sym, kind)


def walks_registry(text, sym, kind):
    name = sym.split("::")[-1]
    return re.search(r"\b" + re.escape(name) + (r"\s*\(" if kind == "fn" else r"\b"), text) is not None


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
        # concat!(env!("CARGO_MANIFEST_DIR"), "/docs/x.md") joins a literal that starts with `/`.
        c = c.lstrip("/")
        if not c or c.startswith("tests/") or ".." in c.split("/") or " " in c:
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
        tok = tok.strip(".").lstrip("/")
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

# The registry set R: every name a registry-definer marker declares that resolves.
# Derived from the target's own tests only, so a fixture is judged on its own definers.
registries = {}
for f, t, m in files:
    if m:
        is_def, names = registry_names(t)
        if is_def:
            for n in names:
                kind = resolve(n)
                if kind:
                    registries.setdefault(n, (kind, f))


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


if "--registries" in FLAGS:
    for n in sorted(registries):
        print(f"registry {n}")
    sys.exit(0)

if "--definers" in FLAGS:
    # `<entry> TAB <definer file> TAB <what it asserts>`: paths, then `registry <sym>` lines
    # (inner-rules.sh prints one rules line per entry from this).
    texts = {f: marker(t) for f, t in definers}

    def asserts(text):
        # a `registry <syms> -- <what>` marker reads as its `<what>`
        return text.split("\u2014", 1)[-1].strip() if text.startswith(REGISTRY) else text

    for a in sorted(defines):
        print(f"{a}\t{defines[a]}\t{asserts(texts[defines[a]])}")
    for n in sorted(registries):
        d = registries[n][1]
        print(f"registry {n}\t{d}\t{asserts(marker(dict(definers).get(d) or '') or '')}")
    sys.exit(0)

if "--artifacts" in FLAGS:
    for a in sorted(defines):
        print(a)
    sys.exit(0)

FIX = "--fix" in FLAGS
bad = 0
# Every registry a marker names must resolve; a follower needs a definer.
for f, t, m in files:
    if not m:
        continue
    is_def, names = registry_names(t)
    for n in names:
        if not resolve(n):
            print(f"contract-lint: {f} names registry {n} but src/ has no pub fn|const|static {n.split('::')[-1]} (stale marker)")
            bad = 1
        elif is_def and not names_it(t, n, resolve(n)):
            print(f"contract-lint: {f} declares registry {n} but never names it -- declare it in the test that walks it")
            bad = 1
        elif not is_def and n not in registries:
            print(f"contract-lint: {f} has no definer for registry {n}; promote the line to 'registry {n}' in the test that owns it")
            bad = 1
if bad:
    print("contract-lint: FAILED")
    sys.exit(1)
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
    walked_reg = next(
        (n for n in sorted(registries) if walks_registry(t, n, registries[n][0])), None
    )
    if walked_reg:
        if FIX:
            print(f"{f}: add  //! contract: reads registry {walked_reg}")
            bad = 1
            continue
        print(f"contract-lint: {f} walks registry {walked_reg} (defined by marked {registries[walked_reg][1]}) -- add the line `//! contract: reads registry {walked_reg}` to its first 10 lines")
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
