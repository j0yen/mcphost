#!/usr/bin/env python3
"""origin-counts.py -- PRD-mcphost-external-class-positive-evidence.

Counts-only, read-only view of tenant origin. Opens
`$MCPHOST_DATA_DIR/mcphost.db` with `?mode=ro` and prints exactly one line:

  external=<n> claimed=<a> returned=<b> unattributed=<u> legacy=<l> synthetic=<s> internal=<i> unknown=<k>

`legacy` is the unattributed rows whose detail starts `legacy-`; `unknown`
is the rows whose origin is outside the enum. No ids, keys, e-mails or IPs.
The same numbers `/healthz` reports as `tenants_by_origin`,
`external_by_evidence` and `origin_unknown`.
"""
import os
import sqlite3
import sys


def main():
    data_dir = os.environ.get("MCPHOST_DATA_DIR")
    if not data_dir:
        sys.exit("origin-counts: MCPHOST_DATA_DIR is not set")
    path = os.path.join(data_dir, "mcphost.db")
    con = sqlite3.connect("file:%s?mode=ro" % path, uri=True)
    known = ("synthetic", "internal", "unattributed", "external")
    by = {}
    for origin, n in con.execute("SELECT origin, COUNT(*) FROM tenants GROUP BY origin"):
        by[origin] = n
    one = lambda sql: con.execute(sql).fetchone()[0]
    claimed = one("SELECT COUNT(*) FROM tenants WHERE origin = 'external' AND origin_detail = 'claimed-email'")
    returned = one("SELECT COUNT(*) FROM tenants WHERE origin = 'external' AND origin_detail LIKE 'returned:%'")
    legacy = one("SELECT COUNT(*) FROM tenants WHERE origin = 'unattributed' AND origin_detail LIKE 'legacy-%'")
    unknown = sum(n for o, n in by.items() if o not in known)
    print(
        "external=%d claimed=%d returned=%d unattributed=%d legacy=%d synthetic=%d internal=%d unknown=%d"
        % (
            by.get("external", 0),
            claimed,
            returned,
            by.get("unattributed", 0),
            legacy,
            by.get("synthetic", 0),
            by.get("internal", 0),
            unknown,
        )
    )


main()
