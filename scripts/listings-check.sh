#!/usr/bin/env bash
# listings-check.sh — PRD-mcphost-agent-findability P0#3. Fetches each
# deploy/listings/manifest.yaml entry's check_url against the live web and
# prints one status line per entry:
#
#   listed <version>              (registry-json only: version matches)
#   listed (stale <version>)      (registry-json only: version mismatch)
#   listed (version unknown)      (text-search: found, no version to check)
#   pending-review                (text-search: not found yet, but `submitted` is set)
#   missing                       (not found, never submitted)
#   error: <reason>               (probe itself failed, e.g. network error)
#
# Exits non-zero only when an entry is genuinely `stale` (AC2) -- missing /
# pending-review / probe errors do not fail the script, since those are
# expected states for a directory with human review latency or a scripted
# probe a site chooses to block.
set -uo pipefail
cd "$(dirname "$0")/.."

MANIFEST="deploy/listings/manifest.yaml"
CURRENT_VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/^version = "([^"]+)"/\1/')"

if [ ! -f "$MANIFEST" ]; then
  echo "listings-check.sh: missing $MANIFEST" >&2
  exit 2
fi
if [ -z "$CURRENT_VERSION" ]; then
  echo "listings-check.sh: could not read version from Cargo.toml" >&2
  exit 2
fi

python3 - "$MANIFEST" "$CURRENT_VERSION" <<'PY'
import json
import sys
import urllib.request
import urllib.error

import yaml

manifest_path, current_version = sys.argv[1], sys.argv[2]

with open(manifest_path, encoding="utf-8") as f:
    entries = yaml.safe_load(f) or []

any_stale = False

def fetch(url, timeout=10):
    req = urllib.request.Request(url, headers={"User-Agent": "mcphost-listings-check/1"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.status, resp.read()
    except urllib.error.HTTPError as e:
        # A well-formed non-2xx (404, 403, ...) is a normal "not found /
        # not accessible" answer, not a probe failure -- handle it the
        # same as any other status code below instead of raising.
        return e.code, e.read()

for entry in entries:
    entry_id = entry.get("id", "?")
    check_kind = entry.get("check_kind")
    check_url = entry.get("check_url")
    submitted = entry.get("submitted")

    try:
        status_code, body = fetch(check_url)
    except (urllib.error.URLError, TimeoutError, OSError) as e:
        print(f"{entry_id}: error: probe failed ({e})")
        continue

    if status_code != 200:
        # non-2xx: treat as not-listed, same downstream handling as a
        # clean 404 -- but keep any body urllib gave us (some sites put
        # useful text on non-200 responses too).
        pass

    if check_kind == "registry-json":
        version = None
        if status_code == 200:
            try:
                data = json.loads(body)
                servers = data.get("servers", [])
                if servers:
                    version = servers[0].get("server", {}).get("version")
            except (json.JSONDecodeError, AttributeError, IndexError):
                version = None
        if version is None:
            print(f"{entry_id}: missing")
        elif version == current_version:
            print(f"{entry_id}: listed {version}")
        else:
            print(f"{entry_id}: listed (stale {version}, current is {current_version})")
            any_stale = True

    elif check_kind == "text-search":
        search_text = entry.get("search_text", "")
        found = status_code == 200 and search_text.encode("utf-8") in body
        if found:
            print(f"{entry_id}: listed (version unknown)")
        elif submitted:
            print(f"{entry_id}: pending-review (submitted {submitted})")
        else:
            print(f"{entry_id}: missing")

    else:
        print(f"{entry_id}: error: unknown check_kind {check_kind!r}")

sys.exit(1 if any_stale else 0)
PY
