#!/usr/bin/env bash
# sandbox-libs-lock.sh -- regenerate sandbox-libs.lock from sandbox-libs.toml
# (PRD-mcphost-sandbox-file-processing-libs R1). Needs network and `uv`.
set -euo pipefail
cd "$(dirname "$0")/.."
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
python3 - <<'PY' > "$tmp"
import tomllib
for lib in tomllib.load(open("sandbox-libs.toml", "rb"))["lib"]:
    print(f"{lib['name']}=={lib['version']}")
PY
uv pip compile "$tmp" --generate-hashes --no-header --no-annotate \
  --python /usr/bin/python3 -q -o sandbox-libs.lock
