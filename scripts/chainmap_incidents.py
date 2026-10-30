#!/usr/bin/env python3
"""Generate tests/fixtures/chainmap_incident_spellings.json.

PRD-mcphost-chain-near-miss-mapping-refused AC 2: every spelling of a step
`args` mapping that a past incident (or this PRD's class) put in front of the
publish-time checker, with the verdict the checker must give it.

    python3 -I scripts/chainmap_incidents.py --sources tests/fixtures/chainmap_incident_sources/
    python3 -I scripts/chainmap_incidents.py --sources tests/fixtures/chainmap_incident_sources/ --check

A row's `source` is a vendored excerpt file; the generator exits non-zero when
the spelling is not found verbatim in it, so the table cannot name a spelling
nobody wrote. `--check` also fails when the committed JSON differs from what
would be generated. Adding a spelling means adding a row to ROWS below, never
editing the JSON by hand. `from_step` is the 0-based index of the step the
mapping sits in, evaluated against tests/fixtures/chainmap_steps_fixture.json.
"""
import argparse
import json
import pathlib
import sys

OUT = pathlib.Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "chainmap_incident_spellings.json"

# (spelling, source file, from_step, expect, reason, canonical)
ROWS = [
    ("$.prev.action", "prev-contract.txt", 2, "refuse", "missing hop", "$.prev.result.action"),
    ("$.prev.result.lead", "prev-contract.txt", 2, "refuse", "unknown field", "$.prev.result.leads"),
    ("$.prev.result.rows", "path-check.txt", 2, "refuse", "unknown field", "$.prev.result.items"),
    ("$.prev.result.nope", "path-check.txt", 2, "refuse", "unknown field", None),
    ("$.steps[2].result.x", "path-check-b.txt", 2, "refuse", "forward reference", None),
    ("$.steps[0].result.count", "path-check-b.txt", 2, "accept", None, "$.steps[0].result.count"),
    ({"from": "steps.0.rows"}, "wo05-dialect.txt", 2, "refuse", "not a path", "$.steps[0].result.rows"),
    ("steps.0.rows", "this-prd-class.txt", 2, "refuse", "not a path", "$.steps[0].result.rows"),
    ("prev.rows", "this-prd-class.txt", 1, "refuse", "not a path", "$.prev.result.rows"),
    ("$prev.result.rows", "this-prd-class.txt", 1, "refuse", "not a path", "$.prev.result.rows"),
    ("input.filters.since", "this-prd-class.txt", 2, "refuse", "not a path", "$.input.filters.since"),
    ({"path": "$.prev.result.x"}, "this-prd-class.txt", 2, "refuse", "path inside a literal object", "$.prev.result.x"),
    ("input.since", "this-prd-class.txt", 2, "accept", None, None),
    ("index.html", "this-prd-class.txt", 2, "accept", None, None),
    (42, "this-prd-class.txt", 2, "accept", None, None),
    ({"from": "a@x.io"}, "this-prd-class.txt", 2, "accept", None, None),
    ({}, "this-prd-class.txt", 2, "accept", None, None),
]


def needle(spelling):
    return spelling if isinstance(spelling, str) else json.dumps(spelling)


def build(sources):
    rows = []
    for spelling, source, from_step, expect, reason, canonical in ROWS:
        path = sources / source
        if not path.is_file():
            sys.exit(f"chainmap_incidents: missing source excerpt {path}")
        body = "\n".join(line for line in path.read_text().splitlines() if not line.startswith("#"))
        if needle(spelling) not in body:
            sys.exit(f"chainmap_incidents: {needle(spelling)!r} not found verbatim in {source}")
        rows.append(
            {
                "spelling": spelling,
                "source": source,
                "from_step": from_step,
                "expect": expect,
                "reason": reason,
                "canonical": canonical,
            }
        )
    return json.dumps(rows, indent=2, ensure_ascii=False) + "\n"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sources", required=True, type=pathlib.Path)
    ap.add_argument("--check", action="store_true")
    args = ap.parse_args()
    text = build(args.sources)
    if args.check:
        if not OUT.is_file() or OUT.read_text() != text:
            sys.exit(f"chainmap_incidents: {OUT} is stale; rerun without --check")
        return
    OUT.write_text(text)


if __name__ == "__main__":
    main()
