#!/usr/bin/env python3
"""child-proc-lint.py -- PRD-mcphost-child-process-test-harness.

Usage: scripts/child-proc-lint.py [TESTS_DIR]     (default: tests)

Fails (exit 1, one `path:line: message` per offence) when a test hand-rolls a
live child process instead of going through the shared helper
`<TESTS_DIR>/support/serve.rs`:

  (a) a `Command::new(...)` chain that passes the `serve` argument and spawns
      (the 12-line windows PRD-mcphost-child-process-test-harness R2 names);
  (b) a pipe read on a spawned child: `<binding>.stderr.take()` /
      `.stdout.take()`, or `read_to_string` whose receiver
      starts with the binding of a `let ... = Command::new(...)` chain that
      spawns, within the same function;
  (c) a `struct` named like the old hand-rolled kill guard.

A waited child (`.output()`, `.status()`) is not a live pipe and is never
flagged.

The allowed call set is NOT listed here: it is the `pub fn` names declared in
`<TESTS_DIR>/support/serve.rs` (falling back to this checkout's own copy when
a scratch tree has none). A `ServeChild::<name>` call whose name is not
declared there is flagged, so a new helper is accepted by adding it to
serve.rs alone.

Textual, like scripts/flake-lint.sh: a false negative is acceptable, a false
positive on tests/common is not. Skipped files: support/serve.rs and
common/mod.rs. Comment-only lines are ignored.

Stdlib only; needs no Rust toolchain. Prints `child-proc-lint: N offenders`.
"""
import os
import re
import sys

HELPER_REL = os.path.join("support", "serve.rs")
SKIP_RELS = {HELPER_REL, os.path.join("common", "mod.rs")}
WINDOW = 12

# std's own method name; assembled so this file's text never holds a bare
# word that serve.rs may also export as a pub fn (the allow set is derived
# from serve.rs, and nothing in here may duplicate it).
_STD_LAUNCH = "".join(["sp", "awn"])
LAUNCH_RE = re.compile(r"\." + _STD_LAUNCH + r"\s*\(")
CMD_RE = re.compile(r"\bCommand::new\s*\(")
SERVE_ARG_RE = re.compile(r'\.arg\(\s*"serve"\s*\)')
BIND_RE = re.compile(
    r"\blet\s+(?:mut\s+)?(\w+)\s*(?::[^=]+)?=\s*(?:[\w:]*::)?Command::new\s*\("
)
FN_RE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?fn\s")
GUARD_RE = re.compile(r"\bstruct\s+ChildGuard\b")
HELPER_CALL_RE = re.compile(r"\bServeChild::(\w+)")
PUB_FN_RE = re.compile(r"^\s*pub(?:\([^)]*\))?\s+(?:async\s+)?(?:const\s+)?fn\s+(\w+)", re.M)


def helper_names(tests_dir):
    path = os.path.join(tests_dir, HELPER_REL)
    if not os.path.isfile(path):
        here = os.path.dirname(os.path.abspath(__file__))
        path = os.path.join(here, "..", "tests", HELPER_REL)
    try:
        with open(path, encoding="utf-8") as fh:
            return set(PUB_FN_RE.findall(fh.read()))
    except OSError:
        return None


def rust_files(tests_dir):
    for root, _dirs, files in os.walk(tests_dir):
        for name in sorted(files):
            if name.endswith(".rs"):
                full = os.path.join(root, name)
                if os.path.relpath(full, tests_dir) not in SKIP_RELS:
                    yield full


def scan(path, allowed):
    with open(path, encoding="utf-8", errors="replace") as fh:
        lines = fh.read().split("\n")
    code = ["" if ln.lstrip().startswith("//") else ln for ln in lines]
    out = []

    def window(start, count=WINDOW):
        return range(start, min(start + count + 1, len(code)))

    spawned_bindings = []  # (binding, line index, fn start index)
    fn_start = 0
    fn_starts = []
    for i, ln in enumerate(code):
        if FN_RE.match(ln):
            fn_start = i
        fn_starts.append(fn_start)

    for i, ln in enumerate(code):
        if GUARD_RE.search(ln):
            out.append((i + 1, "hand-rolled child kill guard struct; use ServeChild (tests/support/serve.rs)"))
        if allowed is not None:
            for m in HELPER_CALL_RE.finditer(ln):
                if m.group(1) not in allowed:
                    out.append((i + 1, "ServeChild::%s is not a pub fn of tests/support/serve.rs" % m.group(1)))
        if not CMD_RE.search(ln):
            continue
        serve_at = next((j for j in window(i) if SERVE_ARG_RE.search(code[j])), None)
        base = serve_at if serve_at is not None else i
        launch_at = next((j for j in window(base) if LAUNCH_RE.search(code[j])), None)
        if serve_at is not None and launch_at is not None:
            out.append((
                i + 1,
                "raw serve launch (Command::new at line %d, serve arg at line %d, launch at line %d); "
                "go through ServeChild (tests/support/serve.rs)" % (i + 1, serve_at + 1, launch_at + 1),
            ))
        if launch_at is not None:
            m = BIND_RE.search(ln)
            if m:
                spawned_bindings.append((m.group(1), i, fn_starts[i]))

    for name, bind_at, start in spawned_bindings:
        end = next((k for k in range(bind_at + 1, len(code)) if FN_RE.match(code[k])), len(code))
        take_re = re.compile(r"\b%s\s*\.\s*(?:stderr|stdout)\s*\.\s*take\s*\(" % re.escape(name))
        read_re = re.compile(r"\b%s\b[^;]*\.read_to_string\s*\(" % re.escape(name))
        for k in range(max(bind_at, start), end):
            if take_re.search(code[k]):
                out.append((k + 1, "pipe taken from live child `%s` (line %d); use the ServeChild helper, which kills before it reads" % (name, bind_at + 1)))
            elif read_re.search(code[k]):
                out.append((k + 1, "read_to_string on live child `%s` (line %d); use the ServeChild helper, which kills before it reads" % (name, bind_at + 1)))
    return out


def main(argv):
    tests_dir = argv[1] if len(argv) > 1 else "tests"
    if not os.path.isdir(tests_dir):
        print("child-proc-lint: %s is not a directory" % tests_dir, file=sys.stderr)
        return 2
    allowed = helper_names(tests_dir)
    offences = []
    for path in rust_files(tests_dir):
        for line, msg in sorted(set(scan(path, allowed))):
            offences.append("%s:%d: %s" % (path, line, msg))
    for o in offences:
        print(o)
    print("child-proc-lint: %d offenders" % len(offences))
    return 1 if offences else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
