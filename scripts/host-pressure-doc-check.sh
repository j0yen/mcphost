#!/usr/bin/env bash
# host-pressure-doc-check.sh -- PRD-mcphost-status-host-pressure requirement 6
# (AC7): docs/metrics.md carries one table of the /status.json `host`
# object's fields, generated from the field list of `HostPressure` in
# src/hostpressure.rs (parsed back out of the Rust source, never a second
# hand-maintained list). A renamed or added field fails the check until the
# table is regenerated; a field with no unit description below fails loudly.
#
# Usage:
#   scripts/host-pressure-doc-check.sh          check: exit 0 and report
#                                                fields=<n> docs_in_sync=1, or
#                                                exit 1 naming the drifted field.
#   scripts/host-pressure-doc-check.sh --write  regenerate the marked block.
#
# Env overrides (tests run the script against a scratch copy):
#   HOST_PRESSURE_SOURCE (default src/hostpressure.rs)
#   HOST_PRESSURE_DOC    (default docs/metrics.md)
set -uo pipefail
cd "$(dirname "$0")/.."

SOURCE="${HOST_PRESSURE_SOURCE:-src/hostpressure.rs}"
DOC="${HOST_PRESSURE_DOC:-docs/metrics.md}"
START="<!-- mcphost-host-pressure:start -->"
END="<!-- mcphost-host-pressure:end -->"
MODE="${1:-check}"
[ "$MODE" = "--write" ] && MODE="write"

for f in "$SOURCE" "$DOC"; do
  if [ ! -f "$f" ]; then
    echo "host-pressure-doc-check.sh: missing $f" >&2
    exit 2
  fi
done

render_block() {
  python3 - "$SOURCE" "$START" "$END" <<'PY'
import re
import sys

source_path, start, end = sys.argv[1:4]
text = open(source_path, encoding="utf-8").read()
m = re.search(r"pub struct HostPressure \{(.*?)\n\}", text, re.S)
if not m:
    print("host-pressure-doc-check.sh: could not find HostPressure in " + source_path, file=sys.stderr)
    sys.exit(2)
fields = re.findall(r"^\s*pub (\w+): ([^,]+),", m.group(1), re.M)
if not fields:
    print("host-pressure-doc-check.sh: HostPressure has no fields", file=sys.stderr)
    sys.exit(2)

DESC = {
    "load1": "1-minute load average (`/proc/loadavg` field 1); unitless",
    "load5": "5-minute load average (`/proc/loadavg` field 2); unitless",
    "psi_cpu_some_avg60": "percent of the last 60 s at least one task stalled on CPU (`/proc/pressure/cpu`, `some` line, `avg60=`); percent",
    "cpu_steal_pct_since_boot": "hypervisor steal since boot: `/proc/stat` first `cpu` line, field 8 / sum of fields x 100; percent",
    "mem_available_mb": "`MemAvailable:` from `/proc/meminfo`, kB / 1024; MiB",
    "nproc": "`std::thread::available_parallelism` (honours cgroup limits); CPUs",
    "sampled_at": "unix time the sample was read, so its age is visible through the 60 s cache; seconds",
}
print(start)
print("| field | type | meaning; unit |")
print("|---|---|---|")
for name, ty in fields:
    if name not in DESC:
        print(f"host-pressure-doc-check.sh: no description for field {name}; add it to DESC", file=sys.stderr)
        sys.exit(2)
    print(f"| `{name}` | `{ty.strip()}` | {DESC[name]} |")
print(end)
PY
}

RENDERED="$(render_block)" || exit $?
FIELD_COUNT=$(printf '%s\n' "$RENDERED" | grep -c '^| `')

n_start=$(grep -Fc "$START" "$DOC")
n_end=$(grep -Fc "$END" "$DOC")
if [ "$n_start" -ne 1 ] || [ "$n_end" -ne 1 ]; then
  echo "host-pressure-doc-check.sh: $DOC must have exactly one $START / $END pair (found start=$n_start end=$n_end)" >&2
  exit 2
fi

if [ "$MODE" = "write" ]; then
  tmp="$(mktemp)"
  python3 - "$DOC" "$START" "$END" "$RENDERED" > "$tmp" <<'PY'
import sys

path, start, end, rendered = sys.argv[1:5]
text = open(path, encoding="utf-8").read()
a = text.index(start)
b = text.index(end) + len(end)
sys.stdout.write(text[:a] + rendered + text[b:])
PY
  mv "$tmp" "$DOC"
  echo "host-pressure-doc-check.sh: wrote fields=${FIELD_COUNT}"
  exit 0
fi

ACTUAL=$(python3 - "$DOC" "$START" "$END" <<'PY'
import sys

path, start, end = sys.argv[1:4]
text = open(path, encoding="utf-8").read()
sys.stdout.write(text[text.index(start):text.index(end) + len(end)])
PY
)
if [ "$ACTUAL" = "$RENDERED" ]; then
  echo "host-pressure-doc-check.sh: fields=${FIELD_COUNT} docs_in_sync=1"
  exit 0
fi
FAILURES=0
while IFS= read -r line; do
  name=$(printf '%s' "$line" | sed -n 's/^| `\([a-z0-9_]*\)`.*/\1/p')
  [ -z "$name" ] && continue
  if ! printf '%s\n' "$ACTUAL" | grep -qF "$line"; then
    echo "host-pressure-doc-check.sh: $DOC is missing or has a stale row for field ${name}" >&2
    FAILURES=$((FAILURES + 1))
  fi
done <<< "$RENDERED"
while IFS= read -r name; do
  if ! printf '%s\n' "$RENDERED" | grep -qF "| \`${name}\` |"; then
    echo "host-pressure-doc-check.sh: $DOC documents field ${name} that the struct no longer has" >&2
    FAILURES=$((FAILURES + 1))
  fi
done < <(printf '%s\n' "$ACTUAL" | sed -n 's/^| `\([a-z0-9_]*\)`.*/\1/p')
[ "$FAILURES" -eq 0 ] && echo "host-pressure-doc-check.sh: $DOC drifted from the generated block; run --write" >&2
exit 1
