#!/usr/bin/env bash
# listings-submit.sh — PRD-mcphost-agent-findability P0#2/#3, AC3/AC4.
# Prepared submissions (deploy/listings/{prs,forms}/*.md, one per
# deploy/listings/manifest.yaml entry) are executed only when a
# `PUBLISH-OK` file is present -- exactly the same gate shape as the
# registry publish (PRD-grand-loop-listings, PRD-mcphost-listing-freshness).
#
# Without PUBLISH-OK: prints what it would submit for every manifest
# entry and exits 0. Submits nothing, writes nothing to the ledger.
#
# With PUBLISH-OK: for each manifest entry, invokes a submitter and
# records one line per entry in deploy/listings/submissions.jsonl with
# {id, method, artifact, date, result}. This repo has no automated
# opener for third-party GitHub PRs or directory web forms (that would
# mean holding credentials for punkpeye/wong2's repos and for four
# directory sites) -- the real submitter is a human using the prepared
# artifact files. So the default submitter, when PUBLISH-OK is present
# and no override is given, records `result: "skipped: no submitter
# configured"` rather than fabricating a submission.
#
# LISTINGS_SUBMITTER, if set, names an executable invoked once per entry
# as `"$LISTINGS_SUBMITTER" <id> <method> <artifact>`; its stdout (if any)
# is captured into the ledger line's `submitter_output` field, and a
# non-zero exit records `result: "failed"` for that entry instead of
# `"submitted"`. This is the injection point PRD acceptance test AC4 uses
# ("a fake submitter") without this script needing to know what a fake
# submitter looks like.
set -uo pipefail
cd "$(dirname "$0")/.."

MANIFEST="deploy/listings/manifest.yaml"
PUBLISH_OK_FILE="${LISTINGS_PUBLISH_OK:-deploy/listings/PUBLISH-OK}"
LEDGER="deploy/listings/submissions.jsonl"

if [ ! -f "$MANIFEST" ]; then
  echo "listings-submit.sh: missing $MANIFEST" >&2
  exit 2
fi

PUBLISH_OK=0
[ -f "$PUBLISH_OK_FILE" ] && PUBLISH_OK=1

python3 - "$MANIFEST" "$LEDGER" "$PUBLISH_OK" "$PUBLISH_OK_FILE" <<'PY'
import datetime
import json
import os
import subprocess
import sys

import yaml

manifest_path, ledger_path, publish_ok, publish_ok_file = sys.argv[1:5]
publish_ok = publish_ok == "1"

with open(manifest_path, encoding="utf-8") as f:
    entries = yaml.safe_load(f) or []

submitter = os.environ.get("LISTINGS_SUBMITTER")
today = datetime.date.today().isoformat()

if not publish_ok:
    print(f"PUBLISH-OK absent ({publish_ok_file}) -- dry run, submitting nothing.")
    for entry in entries:
        print(
            f"  would submit: {entry['id']} ({entry['method']}) "
            f"artifact={entry['artifact']} -> {entry['check_url']}"
        )
    sys.exit(0)

print(f"PUBLISH-OK present ({publish_ok_file}) -- submitting.")
records = []
for entry in entries:
    entry_id = entry["id"]
    method = entry["method"]
    artifact = entry["artifact"]

    if submitter:
        try:
            proc = subprocess.run(
                [submitter, entry_id, method, artifact],
                capture_output=True, text=True, timeout=30, check=False,
            )
            if proc.returncode == 0:
                result = "submitted"
            else:
                result = "failed"
            submitter_output = (proc.stdout or "") + (proc.stderr or "")
        except (OSError, subprocess.SubprocessError) as e:
            result = "failed"
            submitter_output = str(e)
    else:
        result = "skipped: no submitter configured"
        submitter_output = ""

    record = {
        "id": entry_id,
        "method": method,
        "artifact": artifact,
        "date": today,
        "result": result,
        "submitter_output": submitter_output.strip(),
    }
    records.append(record)
    print(f"  {entry_id}: {result}")

with open(ledger_path, "a", encoding="utf-8") as f:
    for record in records:
        f.write(json.dumps(record, sort_keys=True) + "\n")

print(f"recorded {len(records)} submission(s) to {ledger_path}")
PY
