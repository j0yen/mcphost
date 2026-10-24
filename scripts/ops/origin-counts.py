#!/usr/bin/env python3
"""origin-counts.py -- PRD-mcphost-external-class-positive-evidence.

Counts-only, read-only view of tenant origin for the PMF line. Reads the
database under $MCPHOST_DATA_DIR (opened `?mode=ro`) and prints exactly one
line and nothing else (no ids, keys, e-mails or IPs):

  external=<n> claimed=<a> returned=<b> unattributed=<u> legacy=<l> synthetic=<s> internal=<i> unknown=<k>

  external     tenants with origin 'external' (= claimed + returned)
  claimed      external, detail 'claimed-email'
  returned     external, detail 'returned:<n>d'
  unattributed origin 'unattributed'
  legacy       unattributed rows whose detail starts 'legacy-' (the backfill's
               pre-deploy rows, so a pre/post partition identity holds while
               new signups keep arriving)
  synthetic    origin 'synthetic'
  internal     origin 'internal'
  unknown      rows whose origin is outside ORIGINS (drift guard, expected 0)

Run on mcphost-1 without printing the key:
  ssh mcphost-1 'set -a; . /etc/mcphost/env; set +a; python3 -I -' < scripts/ops/origin-counts.py
"""
import os
import sqlite3
import sys

# Must equal `state::ORIGIN_NAMES`; the contract test fails on drift.
ORIGINS = ("synthetic", "internal", "unattributed", "external")


def main():
    data_dir = os.environ.get("MCPHOST_DATA_DIR")
    if not data_dir:
        sys.stderr.write("origin-counts: MCPHOST_DATA_DIR is not set\n")
        return 2
    db_path = os.path.join(data_dir, "mcphost.db")
    conn = sqlite3.connect("file:%s?mode=ro" % db_path, uri=True)
    try:
        by_origin = dict(conn.execute("SELECT origin, COUNT(*) FROM tenants GROUP BY origin"))
        claimed, returned = conn.execute(
            "SELECT COALESCE(SUM(origin_detail = 'claimed-email'), 0), "
            "COALESCE(SUM(origin_detail LIKE 'returned:%'), 0) "
            "FROM tenants WHERE origin = 'external'"
        ).fetchone()
        legacy = conn.execute(
            "SELECT COUNT(*) FROM tenants WHERE origin = 'unattributed' AND origin_detail LIKE 'legacy-%'"
        ).fetchone()[0]
    finally:
        conn.close()
    unknown = sum(n for origin, n in by_origin.items() if origin not in ORIGINS)
    print(
        "external=%d claimed=%d returned=%d unattributed=%d legacy=%d synthetic=%d internal=%d unknown=%d"
        % (
            by_origin.get("external", 0),
            claimed,
            returned,
            by_origin.get("unattributed", 0),
            legacy,
            by_origin.get("synthetic", 0),
            by_origin.get("internal", 0),
            unknown,
        )
    )
    return 0


sys.exit(main())
