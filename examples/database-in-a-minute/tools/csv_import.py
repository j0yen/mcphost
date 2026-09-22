import csv
import io

import mcphost

# `state_ops_per_call_max` (free plan: 200, see src/plans.rs) is the most
# rows one `mcphost.state.insert` call accepts; this tool loops so the
# caller can hand it the whole CSV in one call (requirement 8 / AC9).
BATCH_SIZE = 200


def _coerce(value):
    if value == "":
        return None
    try:
        return int(value)
    except ValueError:
        pass
    try:
        return float(value)
    except ValueError:
        return value


def main(args):
    table = args.get("table")
    if not isinstance(table, str) or not table:
        raise ValueError("table is required")
    csv_text = args.get("csv_text")
    if not isinstance(csv_text, str) or not csv_text:
        raise ValueError("csv_text is required")

    reader = csv.DictReader(io.StringIO(csv_text))
    rows = [{k: _coerce(v) for k, v in raw_row.items()} for raw_row in reader]

    inserted = 0
    for i in range(0, len(rows), BATCH_SIZE):
        batch = rows[i : i + BATCH_SIZE]
        result = mcphost.state.insert(table=table, rows=batch)
        inserted += result["inserted"]

    return {"table": table, "inserted": inserted}
