#!/usr/bin/env bash
# Selftest for the per-test nextest terminator (503 MECH2a). Runs on ryzen7, not CI.
set -euo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fails=0

if grep -q 'terminate-after' "$REPO/.config/nextest.toml"; then
  echo "PASS drift: terminate-after present"
else
  echo "FAIL drift: no terminate-after in .config/nextest.toml"
  fails=$((fails + 1))
fi

T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
mkdir -p "$T/sleeper/src" "$T/sleeper/.config"
cat > "$T/sleeper/Cargo.toml" <<'TOML'
[package]
name = "sleeper"
version = "0.0.0"
edition = "2021"

[workspace]
TOML
cat > "$T/sleeper/src/lib.rs" <<'RS'
#[test]
fn parked_test_sleeps() {
    std::thread::sleep(std::time::Duration::from_secs(600));
}
RS
cp "$REPO/.config/nextest.toml" "$T/sleeper/.config/nextest.toml"
sed -i '/slow-timeout/ s/120s/2s/' "$T/sleeper/.config/nextest.toml"
export CARGO_TARGET_DIR="$T/target"

rc=0
out=$(timeout 300 cargo nextest run --manifest-path "$T/sleeper/Cargo.toml" 2>&1) || rc=$?
if [ "$rc" -ne 0 ] && [ "$rc" -ne 124 ] && grep -q 'TIMEOUT' <<<"$out" && grep -q 'parked_test_sleeps' <<<"$out"; then
  echo "PASS terminator: TIMEOUT parked_test_sleeps"
else
  echo "FAIL terminator: rc=$rc, expected non-zero with TIMEOUT and parked_test_sleeps"
  echo "$out" | tail -20
  fails=$((fails + 1))
fi

sed -i '/slow-timeout/d' "$T/sleeper/.config/nextest.toml"
rc=0
timeout 30 cargo nextest run --manifest-path "$T/sleeper/Cargo.toml" >/dev/null 2>&1 || rc=$?
if [ "$rc" -eq 124 ]; then
  echo "PASS mutation: hung without terminator (rc=124)"
else
  echo "FAIL mutation: expected rc=124, got rc=$rc"
  fails=$((fails + 1))
fi

if [ "$fails" -eq 0 ]; then
  echo "SELFTEST PASS 3/3"
else
  echo "SELFTEST FAIL $fails/3"
  exit 1
fi
