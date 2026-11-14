import time

import mcphost

WINDOW_S = 24 * 60 * 60


def _is_up(code):
    return code is not None and 200 <= code < 300


def main(args):
    targets = mcphost.state.query(table="targets", order_by="added_at asc")["rows"]
    now = time.time()
    out = []
    for t in targets:
        url = t["url"]
        rows = mcphost.state.query(table="checks", where=f"url = '{url}'", order_by="at asc")["rows"]
        if not rows:
            out.append(
                {"url": url, "last_code": None, "last_ms": None, "last_at": None, "up_pct_24h": 0}
            )
            continue

        last = rows[-1]
        recent = [r for r in rows if r["at"] >= now - WINDOW_S]
        up_count = sum(1 for r in recent if _is_up(r["code"]))
        up_pct = round(100 * up_count / len(recent)) if recent else 0

        entry = {
            "url": url,
            "last_code": last["code"],
            "last_ms": last["ms"],
            "last_at": last["at"],
            "up_pct_24h": up_pct,
        }
        if not _is_up(last["code"]):
            # Walk back from the newest row while it's still down; the
            # earliest `at` in that unbroken streak is `down_since`.
            down_since = last["at"]
            for r in reversed(rows):
                if _is_up(r["code"]):
                    break
                down_since = r["at"]
            entry["down_since"] = down_since
        out.append(entry)
    return {"targets": out}
