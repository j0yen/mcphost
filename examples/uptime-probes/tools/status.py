import time

import mcphost

MINUTE_S = 60
DAY_S = 24 * 60 * 60


def main(args):
    # PRD-mcphost-timeseries-buckets R6: every number below is a bucket
    # `host.ts.get` computed; this tool only reads them. The window starts on
    # a whole minute so the bucket timestamps (and `down_since`) do not move
    # between two calls, and ends under a day after it so the 1d bucket is one.
    now = int(time.time())
    start = (now // MINUTE_S - (DAY_S // MINUTE_S - 1)) * MINUTE_S

    out = []
    for url in mcphost.state.get("targets", default=[]):
        tags = {"url": url}
        minutes = mcphost.ts.get("up", "1m", from_=start, tags=tags)["buckets"]
        if not minutes:
            out.append(
                {"url": url, "up": None, "up_pct_24h": 0, "last_at": None, "latency_p95_ms": None}
            )
            continue

        day = mcphost.ts.get("up", "1d", from_=start, tags=tags)["buckets"]
        latency = mcphost.ts.get("latency_ms", "1d", from_=start, tags=tags)["buckets"]
        newest = minutes[-1]
        entry = {
            "url": url,
            "up": newest["max"] >= 1,
            "up_pct_24h": round(100 * day[0]["avg"]),
            "last_at": newest["ts"],
            "latency_p95_ms": latency[0]["p95"] if latency else None,
        }
        if not entry["up"]:
            # Walk back from the newest minute while no probe in it succeeded;
            # the earliest bucket of that unbroken streak is `down_since`.
            down_since = newest["ts"]
            for bucket in reversed(minutes):
                if bucket["max"] >= 1:
                    break
                down_since = bucket["ts"]
            entry["down_since"] = down_since
        out.append(entry)
    return {"targets": out}
