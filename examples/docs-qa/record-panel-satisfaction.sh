#!/usr/bin/env bash
# record-panel-satisfaction.sh -- PRD-mcphost-docs-qa-recipe AC10's
# recorder: turn one synthorg panel run's `measure.json` into the
# `satisfaction[rag_indexer]` line that goes into the vision doc (and this
# repo's own receipt), refusing any run whose corpus did not actually
# include this recipe's `docs_qa_recipe` task -- "reported with the new task
# included" is the whole point of the AC, and a panel number measured
# without the task cannot answer it no matter how good it looks.
#
# The number itself comes from a run this script never performs: a truth-tier
# `synthorg consume` against prod is an operator-authorized, paid, live
# action (~$5/run, ~40 min) and lives in ~/repos/synthorg, not here. What
# this script owns is everything around it that CAN be proven before that
# run: the inclusion check, the truth-vs-offline classification, the target
# comparison, the exact recorded line, and idempotency.
#
# Requires: bash, python3 (stdlib only -- same constraint as docs-qa.sh).
#
# Usage:
#   record-panel-satisfaction.sh --measure <measure.json> \
#       [--vision <vision.md>] [--receipt <receipt.md>] \
#       [--segment rag_indexer] [--task docs_qa_recipe] [--target 0.75]
#
#   --measure   a synthorg run's measure.json (runs/<run>/measure.json).
#   --vision    vision doc to append the dated bullet to (the PRD's own
#               Vision: visions/mcphost-data-layer-first-slice.md in
#               ~/Documents/PRDs). Optional: with neither --vision nor
#               --receipt the script only reports, writing nothing.
#   --receipt   this repo's own copy of the record
#               (docs/receipts/docs-qa-panel-satisfaction.md).
#   --segment   panel segment whose satisfaction is recorded.
#   --task      corpus task that must be in the run for it to count.
#   --target    the PRD's success-metric target, as a fraction.
#
# Exit codes:
#   0  recorded, and it is a live truth-tier run that met the target -- the
#      only combination AC10 accepts as green.
#   1  recorded, but the run is not a live truth-tier run (an offline
#      fake-mode run: `client_version == fake` / a `fake-*` endpoint
#      version) or its number is under the target. The number still lands
#      in the vision and the receipt -- AC10 asks for the number to be
#      recorded, and a miss is a number too.
#   2  refused, nothing written: the run's corpus lacks --task, the panel
#      lacks --segment, it is not a `tier: truth` panel run at all, or the
#      measure.json is unreadable.
set -uo pipefail
# Deliberately no `cd` (unlike docs-qa.sh, which has to find its own
# ./corpus): every path here comes from the caller, so a relative
# --measure/--vision must keep resolving against the caller's cwd.

python3 - "$@" <<'PY'
import datetime
import json
import os
import re
import sys


def usage(msg=None):
    if msg:
        print(f"record-panel-satisfaction.sh: {msg}", file=sys.stderr)
    print(
        "usage: record-panel-satisfaction.sh --measure <measure.json> "
        "[--vision <vision.md>] [--receipt <receipt.md>] [--segment rag_indexer] "
        "[--task docs_qa_recipe] [--target 0.75]",
        file=sys.stderr,
    )
    sys.exit(2)


measure_path = None
vision_path = None
receipt_path = None
segment = "rag_indexer"
task_id = "docs_qa_recipe"
target = 0.75

argv = sys.argv[1:]
while argv:
    arg = argv.pop(0)
    if arg == "--measure" and argv:
        measure_path = argv.pop(0)
    elif arg == "--vision" and argv:
        vision_path = argv.pop(0)
    elif arg == "--receipt" and argv:
        receipt_path = argv.pop(0)
    elif arg == "--segment" and argv:
        segment = argv.pop(0)
    elif arg == "--task" and argv:
        task_id = argv.pop(0)
    elif arg == "--target" and argv:
        try:
            target = float(argv.pop(0))
        except ValueError:
            usage("--target must be a fraction, e.g. 0.75")
    else:
        usage(f"unexpected argument {arg!r}")

if not measure_path:
    usage("--measure is required")


def refuse(msg):
    print(f"REFUSED: {msg}", file=sys.stderr)
    sys.exit(2)


try:
    with open(measure_path, encoding="utf-8") as fh:
        measure = json.load(fh)
except OSError as e:
    refuse(f"cannot read {measure_path}: {e}")
except ValueError as e:
    refuse(f"{measure_path} is not valid JSON: {e}")

if not isinstance(measure, dict):
    refuse(f"{measure_path} is not a measure.json object")

tier = measure.get("tier")
if tier != "truth":
    refuse(
        f"{measure_path} is a {tier!r}-tier run; AC10 records the truth-tier "
        "panel run (synthorg consume --tier truth)"
    )

composition = measure.get("composition") or {}
segments = composition.get("segments") or []
if segment not in segments:
    refuse(f"the run's panel composition does not include {segment!r}: {segments}")

corpus = measure.get("corpus") or {}
task_ids = corpus.get("task_ids") or []
if task_id not in task_ids:
    refuse(
        f"the run's corpus does not include {task_id!r} ({len(task_ids)} tasks) -- "
        "a panel number measured without this recipe's own task cannot answer AC10"
    )

by_segment = (measure.get("satisfaction") or {}).get("by_segment") or {}
if segment not in by_segment:
    refuse(f"the run reports no satisfaction for {segment!r}: {sorted(by_segment)}")
try:
    value = float(by_segment[segment])
except (TypeError, ValueError):
    refuse(f"satisfaction[{segment}] is not a number: {by_segment[segment]!r}")

# A run driven by the in-process fake endpoint (SYNTHORG_LLM_MODE=fake, the
# default) carries `client_version: fake` and a `fake-*` endpoint version.
# It exercises the identical pipeline over the identical corpus, so it
# proves the task is wired in and the number is computable -- but its
# personas and judge are stubs, so it is never the market number the PRD's
# success metric names, and it must never be recorded as if it were.
client_version = str(measure.get("client_version") or "")
endpoint_version = str(measure.get("endpoint_version") or "")
offline = client_version == "fake" or endpoint_version.startswith("fake-")
label = "offline" if offline else "truth-tier"

run_dir = os.path.dirname(os.path.abspath(measure_path))
run_name = os.path.basename(run_dir)
parent = os.path.basename(os.path.dirname(run_dir))
run_ref = f"{parent}/{run_name}" if parent == "runs" else run_dir

scored = (measure.get("scored_by_segment") or {}).get(segment)
sessions = (measure.get("sessions_by_segment") or {}).get(segment)
fingerprint = str(corpus.get("fingerprint") or "")[:12] or "unknown"
today = datetime.date.today().isoformat()

line = (
    f"- {today} {label} run {run_ref}: satisfaction[{segment}]="
    f"{value * 100:.1f}% (target >= {target * 100:.0f}%, scored {scored}/{sessions} "
    f"sessions, {task_id} included, corpus fingerprint {fingerprint})"
)

MARKER = f"## Satisfaction record — satisfaction[{segment}]"
# Idempotent per run directory + segment: the nightly may hand the same
# run to this script more than once (a re-run of the lift step, a retry of
# the cross-repo commit), and the record is a log, not a set of duplicates.
already = re.compile(rf"run {re.escape(run_ref)}: satisfaction\[{re.escape(segment)}\]")


def append(path):
    try:
        with open(path, encoding="utf-8") as fh:
            text = fh.read()
    except FileNotFoundError:
        text = ""
    except OSError as e:
        print(f"record-panel-satisfaction.sh: cannot read {path}: {e}", file=sys.stderr)
        sys.exit(2)

    if already.search(text):
        print(f"ALREADY-RECORDED: {path}")
        return

    lines = text.splitlines()
    if MARKER in lines:
        at = lines.index(MARKER) + 1
        # Land at the END of the marker's own section (just before the next
        # "## " heading, or EOF), so the record reads oldest-first like the
        # vision docs' other dated bullet lists, and trailing blank lines
        # stay trailing.
        while at < len(lines) and not lines[at].startswith("## "):
            at += 1
        while at > 0 and not lines[at - 1].strip():
            at -= 1
        # A bullet directly under prose still parses as a list, but every
        # dated bullet list in these docs is separated by a blank line.
        prev = lines[at - 1] if at > 0 else ""
        lines[at:at] = [line] if (prev.startswith("- ") or not prev.strip()) else ["", line]
    else:
        if lines and lines[-1].strip():
            lines.append("")
        lines.extend([MARKER, "", line])

    try:
        with open(path, "w", encoding="utf-8") as fh:
            fh.write("\n".join(lines).rstrip("\n") + "\n")
    except OSError as e:
        print(f"record-panel-satisfaction.sh: cannot write {path}: {e}", file=sys.stderr)
        sys.exit(2)
    print(f"RECORDED: {path}")


print(f"SEGMENT: {segment}")
print(f"TIER: {label}")
print(f"TASK_INCLUDED: {task_id}")
print(f"SATISFACTION: {value}")
print(f"TARGET: {target}")
print(f"LINE: {line}")

for path in (vision_path, receipt_path):
    if path:
        append(path)

if offline:
    print(
        f"NOT-GREEN: an offline fake-mode run cannot stand in for the truth-tier "
        f"panel run AC10 names (client_version={client_version!r}, "
        f"endpoint_version={endpoint_version!r})"
    )
    sys.exit(1)
if value < target:
    print(f"NOT-GREEN: satisfaction[{segment}]={value * 100:.1f}% is under the {target * 100:.0f}% target")
    sys.exit(1)
print("GREEN: truth-tier panel run met the target with the new task included")
PY
