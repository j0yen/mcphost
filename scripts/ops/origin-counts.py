#!/usr/bin/env python3
"""origin-counts.py -- PRD-mcphost-external-class-positive-evidence.

Counts-only, read-only view of the tenant origin classes. Reads the database
path from MCPHOST_DATA_DIR (<dir>/mcphost.db), opens it `mode=ro`, and prints
exactly one line:

  external=<n> claimed=<a> returned=<b> unattributed=<u> legacy=<l> synthetic=<s> internal=<i> unknown=<k>

`legacy` = unattributed rows whose detail starts `legacy-`. `unknown` = rows whose
origin is outside ORIGINS (drift guard, expected 0). Nothing else is printed:
no ids, keys, e-mails or IPs. tests/mcphost-external-class-positive-evidence_ac07_*
keeps ORIGINS equal to the Rust `Origin` enum.

Run on the prod box:  ssh mcphost-1 'set -a; . /etc/mcphost/env; set +a; python3 -I -' < scripts/ops/origin-counts.py
"""
import os
import sqlite3
import sys

ORIGINS = ("synthetic", "internal", "unattributed", "external")


def main():
    data_dir = os.environ.get("MCPHOST_DATA_DIR")
    if not data_dir:
        sys.exit("origin-counts: MCPHOST_DATA_DIR is not set")
    db = sqlite3.connect("file:" + os.path.join(data_dir, "mcphost.db") + "?mode=ro", uri=True)
    try:
        one = lambda sql, *args: db.execute(sql, args).fetchone()[0] or 0
        by_origin = {name: one("SELECT COUNT(*) FROM tenants WHERE origin = ?", name) for name in ORIGINS}
        marks = ",".join("?" * len(ORIGINS))
        unknown = one("SELECT COUNT(*) FROM tenants WHERE origin NOT IN (%s)" % marks, *ORIGINS)
        claimed = one("SELECT COUNT(*) FROM tenants WHERE origin = 'external' AND origin_detail = 'claimed-email'")
        returned = one("SELECT COUNT(*) FROM tenants WHERE origin = 'external' AND origin_detail LIKE 'returned:%'")
        legacy = one("SELECT COUNT(*) FROM tenants WHERE origin = 'unattributed' AND origin_detail LIKE 'legacy-%'")
    finally:
        db.close()
    print(
        "external=%d claimed=%d returned=%d unattributed=%d legacy=%d synthetic=%d internal=%d unknown=%d"
        % (by_origin["external"], claimed, returned, by_origin["unattributed"], legacy,
           by_origin["synthetic"], by_origin["internal"], unknown)
    )


main()
