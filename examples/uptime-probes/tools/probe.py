import json
import os
import time
import urllib.error
import urllib.request

import mcphost

# Requirement 2/AC5: never probe more than this many targets in one run,
# even if `targets` holds more -- the rest are reported, not silently
# dropped (see `main`'s `capped`/`total_targets`).
MAX_TARGETS = 20
# Requirement 1: 10 s per-URL timeout.
FETCH_TIMEOUT_S = 10
NOTIFY_TIMEOUT_S = 5
# Points older than this are trimmed from both series every run.
RETENTION_S = 24 * 60 * 60
# PRD-mcphost-timeseries-buckets R6: one point per target per run in each
# series, tagged with the url. `status` reads them back with `host.ts.get`.
UP_SERIES = "up"
LATENCY_SERIES = "latency_ms"


def _is_up(code):
    return code is not None and 200 <= code < 300


def _fetch(url):
    start = time.monotonic()
    try:
        with urllib.request.urlopen(url, timeout=FETCH_TIMEOUT_S) as resp:
            code = resp.getcode()
    except urllib.error.HTTPError as e:
        # A non-2xx response still has a real status code -- that's a
        # down target, not a tool exception.
        code = e.code
    except Exception:
        code = 0
    ms = (time.monotonic() - start) * 1000
    return code, ms


def _notify_owner(url, code):
    """P2 hand-off to the agent-inbox recipe (requirement 8/AC9): a
    best-effort `host.msg.send` to this tool's own owner, over the same
    `network: public` egress `_fetch` uses. Composition (`mcphost.call`)
    can't reach a `host.*` control-plane tool -- only a tenant's own
    published tools -- so this goes over plain HTTP instead, authenticated
    with the owner's own key (published as the `self_key` secret) and
    reaching the same endpoint this tenant signed up against (published as
    the `ENDPOINT_URL` plain env var). Silently gives up on any failure --
    a probe run's own result must never depend on this succeeding.
    """
    endpoint = os.environ.get("ENDPOINT_URL")
    namespace = os.environ.get("OWNER_NAMESPACE")
    key = os.environ.get("SECRET_SELF_KEY")
    if not endpoint or not namespace or not key:
        return
    word = "up" if _is_up(code) else "down"
    payload = json.dumps(
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "host.msg.send",
                "arguments": {
                    "to": [namespace],
                    "body": f"uptime-probes: {url} is now {word} (code {code})",
                },
            },
        }
    ).encode()
    req = urllib.request.Request(
        endpoint,
        data=payload,
        headers={
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
            "Authorization": f"Bearer {key}",
        },
        method="POST",
    )
    try:
        urllib.request.urlopen(req, timeout=NOTIFY_TIMEOUT_S)
    except Exception:
        pass


def _was_up(url):
    """The last recorded state of `url`, from the newest minute bucket of its
    `up` series. A target with no history is assumed up, so a target that is
    down from its very first check still flips (and notifies) rather than
    needing a second, confirming failure."""
    buckets = mcphost.ts.get(UP_SERIES, "1m", from_="-24h", tags={"url": url})["buckets"]
    return True if not buckets else buckets[-1]["max"] >= 1


def main(args):
    all_targets = mcphost.state.get("targets", default=[])
    total_targets = len(all_targets)
    capped = total_targets > MAX_TARGETS
    targets = all_targets[:MAX_TARGETS]

    for url in targets:
        prev_up = _was_up(url)
        code, ms = _fetch(url)
        up = _is_up(code)
        mcphost.ts.put(UP_SERIES, 1 if up else 0, tags={"url": url})
        mcphost.ts.put(LATENCY_SERIES, ms, tags={"url": url})
        if up != prev_up:
            _notify_owner(url, code)

    # Requirement 2/AC4: retention runs every call, regardless of how many
    # targets exist this run.
    cutoff = int(time.time() - RETENTION_S)
    for series in (UP_SERIES, LATENCY_SERIES):
        mcphost.ts.trim(series, before_ts=cutoff)

    return {"probed": len(targets), "total_targets": total_targets, "capped": capped}
