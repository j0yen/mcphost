# AC7 cross-repo evidence — the synthorg journey + tool-surface conformance harnesses

AC7: *Given the synthorg journey and tool-surface conformance harnesses at
the landing commit, When run against a build of this PRD, Then no
assertion expects `tenant_key_missing` on `/mcp` and both runs are green
on the fixture host.*

`~/repos/synthorg` is a separate repository outside this worktree's own
`cargo test` gate (no Rust toolchain dependency on it either way — it's a
Python repo). The two harnesses AC7 names are real, identifiable modules
in it, not an unreachable external system:

| harness | module | PRD |
|---|---|---|
| "the synthorg journey harness" | `src/synthorg/journey_client.py` | PRD-synthorg-mcphost-customer-journey-conformance |
| "the tool-surface conformance harness" | `src/synthorg/toolconf.py` | PRD-synthorg-mcphost-tool-surface-conformance |

Both run entirely hermetically against an in-process fake transport
(`tests/journey_fixture.py`'s `FakeJourneyHost`, `tests/toolconf_fixture.py`'s
`FakeToolconfHost` — the "fixture host" AC7's own wording names), so "run
against a build of this PRD" means pinning the fixture's own scripted
behavior to match this PRD's new contract, not standing up a live mcphost
deploy.

## What was found on inspection

The journey harness's 12 core steps never probe an anonymous/bare call at
all (its first step is always a real `signup`) — it carries no assertion
about `tenant_key_missing` on a bare call. The tool-surface conformance
harness's `no_key` probe is exactly that assertion:
`toolconf._EXPECTED_CODES["no_key"] == frozenset({"tenant_key_missing"})`,
scoring a tool that refused a keyless call with that code a **pass**, and
scoring a keyless 200 (what this PRD's implicit signup now does for every
`host.*`/`billing.*` tool) an unconditional **fail cause=leak**. This is
precisely the obsolete assumption AC7's own text calls out.

## Fix (synthorg commit `7b610cb39c9d0e7761c5cd8346b2bc8b5243712f`, on `master`, committed but not pushed)

`src/synthorg/toolconf.py`'s `no_key` probe now gets the identical
self-scoped exemption `foreign_key` already has: a keyless 200 is a leak
only if the body actually names one of the probe's own pre-existing
tenants, never for naming a freshly implicitly-signed-up one (new verdict
`"pass (implicit_signup)"`). A keyless refusal of any kind — the old
`tenant_key_missing` included — is simply wrong now, so `no_key` carries
no expected refusal code/text pattern any more. Every pinned test that
encoded the old contract was updated to prove the new one instead
(`tests/toolconf_hotfix_4xx_code_truth_test.py`,
`tests/toolconf_hotfix_leak_selfscoped_test.py`,
`tests/toolconf_raw_transport_test.py`, `tests/toolconf_ac01_all_pass_test.py`,
`tests/toolconf_fixture.py`'s own fake).

## Hand-run commands (outside this worktree's own gate)

```
$ cd ~/repos/synthorg && uv run pytest -q tests/ -k toolconf
........................................................................ [ 85%]
............                                                             [100%]
84 passed, 2054 deselected in 37.33s

$ cd ~/repos/synthorg && uv run ruff check src/synthorg/toolconf.py tests/toolconf_fixture.py \
    tests/toolconf_ac01_all_pass_test.py tests/toolconf_hotfix_4xx_code_truth_test.py \
    tests/toolconf_hotfix_leak_selfscoped_test.py tests/toolconf_raw_transport_test.py
All checks passed!

$ cd ~/repos/synthorg && uv run mypy src/synthorg/toolconf.py
Success: no issues found in 1 source file
```

`84 passed` is every test file in the repo whose name matches `toolconf`
(confirmed by `grep -rl "import toolconf\|toolconf_fixture" tests/*.py` —
every such file's name contains `toolconf_`, so the `-k toolconf` filter
covers the harness's full blast radius, journey/explore/compete modules
included since none of them import `toolconf.py`).
