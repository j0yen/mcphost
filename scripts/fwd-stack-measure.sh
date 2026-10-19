#!/usr/bin/env bash
# fwd-stack-measure.sh -- PRD-mcphost-generated-dispatch-table-stack-budget R3/R7/R8.
#
# The measurements PR #413 ran by hand, kept in-tree so the next frame change is
# re-measured by running a command, not by re-deriving the method.
#
#   scripts/fwd-stack-measure.sh --frame <fn>
#       The poll frame of `McpHostHandler::<fn>`'s future in the debug binary
#       (`target/debug/mcphost`, built here if absent): the biggest stack
#       allocation in the prologue of the async fn's body closure
#       (`<fn>::{closure#0}` and the `::{closure#N}` chain under it), read from
#       the disassembly, so it is the bytes every forwarded call pays while that
#       future is being polled. Prints `frame <fn> <bytes>`.
#   scripts/fwd-stack-measure.sh [--release] <call|test>:<host.verb>...
#       The minimum stack, in bytes (4 KiB steps), on which the guard
#       `tests/verbfwd_ac08_forward_stack_budget.rs` completes that id's real
#       forwarded call: a binary search over the guard's own child mode
#       (MCPHOST_FWD_STACK_VERBS / MCPHOST_FWD_STACK_BYTES; SIGABRT = overflow,
#       `FWD-DONE` = fits). `--release` measures the release test profile.
#       Prints `min <id> <bytes>` per id.
#   scripts/fwd-stack-measure.sh [--release] all
#       The same for every id the guard runs, then PR #413's markdown table and
#       `max <bytes> <id>` (set FWD_BASELINE=<file holding an earlier run's
#       output> to add its column). FWD_JOBS (default 4) ids are searched at once.
#
# Env: MCPHOST_BIN (frame mode binary), FWD_SUITE (default suite_core_08, the suite
# binary that includes the guard).
set -uo pipefail
cd "$(dirname "$0")/.."

PROFILE_FLAG=()
PROFILE=debug
MODE=""
IDS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --release)
      PROFILE_FLAG=(--release)
      PROFILE=release
      ;;
    --frame)
      MODE=frame
      shift
      FRAME_FN="${1:-}"
      ;;
    all) MODE=all ;;
    call:* | test:*) IDS+=("$1") ;;
    *)
      echo "usage: fwd-stack-measure.sh --frame <fn> | [--release] (all | <call|test>:<verb>...)" >&2
      exit 2
      ;;
  esac
  shift
done
[ -z "$MODE" ] && [ ${#IDS[@]} -gt 0 ] && MODE=ids
[ -z "$MODE" ] && {
  echo "usage: fwd-stack-measure.sh --frame <fn> | [--release] (all | <call|test>:<verb>...)" >&2
  exit 2
}

if [ "$MODE" = frame ]; then
  [ -n "${FRAME_FN:-}" ] || { echo "fwd-stack-measure.sh: --frame needs a function name" >&2; exit 2; }
  BIN="${MCPHOST_BIN:-target/debug/mcphost}"
  if [ ! -x "$BIN" ]; then
    cargo build --locked --bin mcphost >&2 || exit 2
  fi
  exec python3 -I - "$BIN" "$FRAME_FN" <<'PY'
import re, subprocess, sys

binary, fn = sys.argv[1], sys.argv[2]
body = re.compile(r"^<mcphost::handler::McpHostHandler>::" + re.escape(fn) + r"::\{closure#\d+\}(::\{closure#\d+\})*$")
syms = []
for line in subprocess.run(["nm", "-C", binary], capture_output=True, text=True, check=True).stdout.splitlines():
    m = re.match(r"([0-9a-f]+) [tT] (.*)$", line)
    if m and body.match(m.group(2)):
        syms.append((int(m.group(1), 16), m.group(2)))
if not syms:
    sys.exit(f"fwd-stack-measure.sh: no poll function for `{fn}` in {binary} (is it an async fn of McpHostHandler?)")

def frame(addr):
    dis = subprocess.run(
        ["objdump", "-d", "--no-show-raw-insn", f"--start-address=0x{addr:x}", f"--stop-address=0x{addr + 0x80:x}", binary],
        capture_output=True, text=True, check=True,
    ).stdout.splitlines()
    insns = [l.split("\t", 1)[1].strip() for l in dis if re.match(r"\s+[0-9a-f]+:\t", l)]
    pushes = total = probe = 0
    for ins in insns[:30]:
        if ins.startswith("push "):
            pushes += 8
        m = re.match(r"sub\s+\$0x([0-9a-f]+),%r11", ins)  # inline stack probe: r11 = rsp - N
        if m:
            probe = int(m.group(1), 16)
        m = re.match(r"sub\s+\$0x([0-9a-f]+),%rsp", ins)
        if m:
            v = int(m.group(1), 16)
            if not (v == 0x1000 and probe):  # the probe loop's own step
                total += v
        if ins.startswith(("call", "jmp")):
            break
    return pushes + total + probe

best = max((frame(a), n) for a, n in syms)
print(f"frame {fn} {best[0]}")
PY
fi

# ---- minimum-stack modes: binary search over the guard's child mode ----
SUITE="${FWD_SUITE:-$(grep -l 'verbfwd_ac08_forward_stack_budget' tests/suite_*.rs | head -1 | xargs -r basename -s .rs)}"
[ -n "$SUITE" ] || { echo "fwd-stack-measure.sh: no tests/suite_*.rs includes the guard" >&2; exit 2; }
EXE="$(cargo test "${PROFILE_FLAG[@]}" --locked --test "$SUITE" --no-run --message-format=json 2>/dev/null |
  python3 -I -c '
import json, sys
for line in sys.stdin:
    try:
        m = json.loads(line)
    except ValueError:
        continue
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["target"]["name"] == sys.argv[1]:
        print(m["executable"])
' "$SUITE" | tail -1)"
[ -n "$EXE" ] && [ -x "$EXE" ] || { echo "fwd-stack-measure.sh: could not build the $SUITE test binary ($PROFILE)" >&2; exit 2; }

export FWD_EXE="$EXE" FWD_MODE="$MODE" FWD_JOBS="${FWD_JOBS:-4}" FWD_BASELINE="${FWD_BASELINE:-}"
exec python3 -I - "${IDS[@]}" <<'PY'
import os, re, subprocess, sys
from concurrent.futures import ThreadPoolExecutor

exe = os.environ["FWD_EXE"]
TEST = "verbfwd_ac08_forward_stack_budget::every_dotted_verb_forwards_within_the_stack_budget"
STEP = 4096

def fits(ids, stack):
    env = dict(os.environ, MCPHOST_FWD_STACK_VERBS=ids, MCPHOST_FWD_STACK_BYTES=str(stack), RUST_BACKTRACE="0")
    out = subprocess.run([exe, "--exact", TEST, "--nocapture", "--test-threads=1"], env=env,
                         capture_output=True, text=True, stdin=subprocess.DEVNULL)
    return out.returncode == 0 and f"FWD-DONE {ids}" in out.stdout

def minimum(ident):
    lo, hi = 64 * 1024, 4 * 1024 * 1024  # lo: does not fit (checked below), hi: fits
    if fits(ident, lo):
        return lo
    if not fits(ident, hi):
        sys.exit(f"fwd-stack-measure.sh: {ident} does not complete even on {hi} bytes")
    while hi - lo > STEP:
        mid = (lo + hi) // 2 // STEP * STEP
        if mid <= lo:
            break
        if fits(ident, mid):
            hi = mid
        else:
            lo = mid
    return hi

def all_ids():
    env = dict(os.environ, MCPHOST_FWD_STACK_VERBS="all", MCPHOST_FWD_STACK_BYTES=str(8 * 1024 * 1024), RUST_BACKTRACE="0")
    out = subprocess.run([exe, "--exact", TEST, "--nocapture", "--test-threads=1"], env=env,
                         capture_output=True, text=True, stdin=subprocess.DEVNULL).stdout
    ids = re.findall(r"FWD-DONE (\S+)", out)
    if not ids:
        sys.exit("fwd-stack-measure.sh: the guard's child mode ran no id")
    return ids

mode = os.environ["FWD_MODE"]
ids = all_ids() if mode == "all" else sys.argv[1:]
jobs = int(os.environ["FWD_JOBS"]) if mode == "all" else 1
with ThreadPoolExecutor(max_workers=jobs) as pool:
    mins = list(pool.map(minimum, ids))
if mode != "all":
    for ident, n in zip(ids, mins):
        print(f"min {ident} {n}")
    sys.exit(0)

before = {}
if os.environ.get("FWD_BASELINE"):
    for line in open(os.environ["FWD_BASELINE"]):
        m = re.match(r"\| `?([^`| ]+)`? \| (\d+)", line)
        if m:
            before[m.group(1)] = int(m.group(2))
cols = " | before |" if before else " |"
print(f"| id | min stack (B){cols}")
print("|---|---|" + ("---|" if before else ""))
for ident, n in zip(ids, mins):
    print(f"| {ident} | {n} |" + (f" {before.get(ident, '-')} |" if before else ""))
top = max(zip(mins, ids))
print(f"max {top[0]} {top[1]}")
PY
