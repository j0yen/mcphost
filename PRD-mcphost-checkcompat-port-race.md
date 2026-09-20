# PRD — mcphost-checkcompat-port-race: check-compat cannot mistake another process's server for the previous binary

- Status: queued
- Lane: orch 2026-09-20T08:15:13.163240128+00:00 run=34
- build_target: rust-extend
- build_into: /home/jsy/wintermute/mcphost
- build_priority: normal
- Grounding: failure-derived — burst prove 2026-09-15 05:56Z: checkcompat_ac02_ac03 returned exit 0 for a broken previous binary (port race); 2026-09-16 07:51Z redeploy of 0.54.1 logged compat_check=fail with its reason discarded
- build_version_bump: patch
- publish: none
- test_prefix: checkcompat_race
- Vision: visions/buildloop-operations.md
- PM: Joe
- Drafted: 2026-09-15
- Operator-authorization: Joe 2026-09-15T06:17Z "authorize box ACs. stop deferring without operator authorization. you have it. dont stop -- go!" scope: every real-box AC in this PRD — boot Hetzner burst boxes (ccx43/ccx53, up to BURST_MAX_BOXES) under the existing money guards (ttl, idle-guard, disk floors), run the AC, tear down; do not defer for scope or risk.
- deferred_acs: []
- mock_justifications: none — AC8 is no longer deferred. The earlier deferral claimed "this coding sandbox has no HCLOUD_TOKEN"; that was wrong. `wm-build` on this host owns a live Hetzner credential (`~/.config/wm-build/hcloud.token`; a live `GET /v1/servers` returns HTTP 200), so under the Operator-authorization line above a real 32-core ccx53 was booted, this PRD's HEAD synced to it, the prove run there, and the box torn down. `burst-lane`'s installed CLI still has no `prove` verb, so the prove ran through the routing this repo actually uses (`wm-build runner exec -- cargo ...`, which executes cargo on the remote box and never locally). Receipt: docs/benchmarks/checkcompat-race-ccx53-prove.txt, locked by tests/checkcompat_race_ac08_real_ccx53_prove.rs.
- Engineering target: extend ~/wintermute/mcphost — src/compat_check.rs (free_loopback_port, spawn_previous, wait_ready), tests/checkcompat_ac02_ac03.rs, new tests/checkcompat_race_ac*.rs

## TL;DR

`check_compat` no longer frees a loopback port and hopes the child binds it. It binds the listener itself, hands the socket to the previous binary as an inherited fd (`LISTEN_FDS` convention, already how systemd starts mcphost), and the readiness probe requires the child's own identity token in the `/healthz` response. A test that spawns `/bin/false` as the previous binary then gets exit 4 every time, on any core count.

## Problem statement

On 2026-09-15 05:56Z the burst lane's first ccx53 prove ran `cargo test --workspace` on mcphost 9a6fa6e and `checkcompat_ac02_ac03::check_compat_fails_naming_the_step_when_the_previous_binary_cannot_come_up` failed: `check-compat should exit 4 on a broken previous binary, got Some(0)` (tests/checkcompat_ac02_ac03.rs:134). The same commit passed on a 16-core ccx43 minutes earlier and passed on the ccx53 rerun. Cause (src/compat_check.rs:155–202): `free_loopback_port` binds `127.0.0.1:0`, reads the port, drops the listener; `spawn_previous` starts the binary with `MCPHOST_BIND=127.0.0.1:{port}`; `wait_ready` polls `http://127.0.0.1:{port}/healthz` for 10 s. Between the drop and the child's bind, a parallel test's `free_loopback_port` can receive the same port and its real mcphost instance answers the probe, so a `/bin/false` "previous binary" reads as up. More cores → more parallel test threads → higher collision rate. Consequence: casper's prove contract is `cargo test --workspace` exit 0; a flaky mcphost test costs a box-hour (€1.01 on ccx53) and blocks `enable` on every image refresh until someone reruns it. This is an mcphost hermeticity defect, not a lane defect; PRD-mcphost-compat-check-env (shipped 2026-09-08) made the check run under the box's env contract but left port selection racy.

## Goals

- The readiness probe can only be satisfied by the process check-compat spawned.
- No window between choosing a port and binding it.
- The existing exit codes (4 on cannot-come-up) and messages stay the same for callers (mcphost-deploy).

## Non-goals

- Changing the deploy flow around check-compat (mcphost-deploy).
- Serializing mcphost's whole test suite (the race is in one helper).
- Removing `MCPHOST_BIND` support for callers that still pass a port.

## User stories

1. As the burst lane, when I run `cargo test --workspace` on a 32-core box, the check-compat tests pass for the same reason they pass on 16 cores.
2. As mcphost-deploy, when the previous binary cannot start, I still get exit 4 and the step name.
3. As a test author, when I spawn `/bin/false` as the previous binary, no other test's server can answer for it.
4. As the operator, when the compat probe succeeds, the log names the child's pid and the identity token that matched.

## Requirements

1. P0 — `free_loopback_port` is replaced by `bind_loopback_listener() -> TcpListener` bound to `127.0.0.1:0`; the listener is kept open and passed to the child as an inherited socket via `LISTEN_FDS=1` / `LISTEN_PID=<child>` (the systemd socket-activation convention mcphost's server already honors for its production unit; if it does not, this requirement adds that support to the bind path first).
2. P0 — `spawn_previous` generates a random 128-bit `MCPHOST_COMPAT_TOKEN` per invocation, passes it in the child's env, and `wait_ready` accepts `/healthz` only when the response carries header `X-Mcphost-Compat-Token` equal to it; any other 200 is logged `healthz answered without our token (foreign server on port N)` and treated as not-ready.
3. P0 — A child that exits before readiness short-circuits `wait_ready` (poll `try_wait()` each iteration) so `/bin/false` fails in one poll interval, not after the 10 s timeout, and exit code 4 names the step and the child's exit status.
4. P0 — Callers that set `MCPHOST_BIND` explicitly keep the old bind-by-address path but still get the token check.
5. P1 — The healthz handler adds the token header only when `MCPHOST_COMPAT_TOKEN` is set in its env; production responses are unchanged.
6. P2 — `compat_check` logs the chosen port, child pid, and whether the socket was inherited or bound by address, at info level.

7. P0 — The deploy tool stops discarding the check's reason. `mcphost migrate --check-compat` already names the failing step on stderr (tests/checkcompat_ac02_ac03.rs), but `mcphost-deploy` keeps only the verdict: `compat_check.py:86-88` stores the stderr as `detail` and `cli.py:254` prints `compat_check: fail` alone, so the 2026-09-16 03:51 EDT redeploy of 0.54.1 recorded `compat_check=fail` with no way to tell a real schema break from this PRD's race. One cross-repo edit to `~/wintermute/mcphost-deploy` (`cli.py`, `gate_check.append_journal`): on any non-pass verdict the CLI prints `compat_check: fail (step=<step> detail=<first stderr line, 200 chars>)` — the leading `compat_check: fail` token is unchanged so `vibeloop-measure.sh:356` keeps scraping it — and writes `compat-check  fail  (step=... range=<prev>..<new>)` to the gate journal; `doctor` shows the last such line.

## Success metrics

| metric | baseline (2026-09-15) | target | method | timeframe |
|---|---|---|---|---|
| `checkcompat_ac02_ac03` failures per 200 parallel runs on 32 threads | ≥1 observed in 2 real runs | 0 | stress test (AC5) | at ship |
| casper prove failures attributed to mcphost tests | 1 of 2 ccx53 proves | 0 | burst-lane.log | next 5 proves |
| time for `/bin/false` case to fail | up to 10 s | < 1 s | test timing | at ship |

## Technical considerations

- The mcphost server's bind code must accept an inherited fd: check `src/main.rs` / the bind helper for `LISTEN_FDS` handling before choosing between requirement 1's two branches; document which branch was taken in the receipt.
- The token header is added in the healthz handler behind an env check so tenants never see it.
- The stress test must run with `--test-threads=32` regardless of host cores (`cargo test -- --test-threads=32`), so it exercises the race on RedBaron too.
- Keep `test_prefix: checkcompat_race` files self-contained; do not edit `checkcompat_ac02_ac03.rs` beyond replacing the helper it calls.

## Migration / compatibility

None for tenants. mcphost-deploy calls `check_compat` through the same function signature; exit codes unchanged.

## Open questions

| question | owner | due |
|---|---|---|
| Does the production unit already socket-activate (then req 1 is inheritance only) or bind by address (then req 1 adds inheritance)? | builder | before req 1 |
| Should the token check also guard the deploy-time compat probe against a stale previous instance still bound on the real port? | Joe | after ship |

## Acceptance criteria

1. P0 — Given a previous binary of `/bin/false`, When `check_compat` runs, Then it returns exit 4 naming step `previous-up` within 1 s and the log records the child's exit status.
2. P0 — Given a foreign HTTP server already answering 200 on the port check-compat chose (test binds its own listener with `SO_REUSEADDR` on the same port before the child starts, or injects a fake responder), When `wait_ready` polls, Then it does not report ready and the log contains `foreign server on port`.
3. P0 — Given a real previous mcphost binary, When it is spawned with the inherited listener and the token env, Then `/healthz` carries `X-Mcphost-Compat-Token` matching and `check_compat` reports ready.
4. P0 — Given `MCPHOST_COMPAT_TOKEN` unset, When `/healthz` is requested, Then no `X-Mcphost-Compat-Token` header is present.
5. P0 — Given `cargo test -- --test-threads=32` running the two ac02/ac03 tests and 30 dummy tests that each spawn a real mcphost on a fresh listener, When the suite runs 200 times in a loop on RedBaron, Then 0 failures.
6. P1 — Given `MCPHOST_BIND=127.0.0.1:<explicit port>`, When `check_compat` runs against a real previous binary, Then it binds by address (log says so) and still enforces the token.
7. P0 — Given the whole mcphost test suite, When `cargo test --workspace` runs on RedBaron, Then it exits 0 and clippy is unchanged versus the committed baseline (delta-pass convention).
8. P0 — Given a real ccx53 burst box (deferrable only with a justification naming why no box was reachable; `mock_justifications` required), When `burst-lane.sh prove` runs on this PRD's HEAD, Then `prove done routed=true` and the run log shows the ac02/ac03 tests `ok`.
9. P0 — Given `mcphost migrate --check-compat` exits non-zero with stderr `step=signup: connection refused`, When `mcphost-deploy redeploy` runs with a fake runner returning that result, Then stdout contains `compat_check: fail (step=signup detail=step=signup: connection refused` and the gate journal gains one `compat-check  fail  (step=signup` line; Given exit 0, Then stdout is exactly `compat_check: pass` as today.
10. P0 — Given the ledger scraper `grep -o 'compat_check: [a-z]*' | head -1 | awk '{print $2}'` from vibeloop-measure.sh, When it reads the AC9 fail line, Then it yields `fail`, and on the pass line `pass`.
- iter_log: 2026-09-15T22:24:30Z amended by /dream (Joe 2026-09-15 "go 4 wide on different branches", "prioritize the build-skill prds") — build_priority high→low: proof-receipt wedged wall=1800 cpu=0 (real loopback-port hang, 3 blocks on 09-15); admitting it at high spends a mcphost slot that a shippable PRD needs
- iter_log: 2026-09-16T08:35:00Z amended by /dream (Joe 2026-09-16 "amend", after the 0.54.1 redeploy logged compat_check=fail with its reason discarded) — build_priority low→normal; Requirement 7 + AC9, AC10 appended (journal the failing step; cross-repo edit to mcphost-deploy cli.py); ACs 1–8 unchanged
