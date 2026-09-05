# Audit advisory notes

Per-finding explanations for the `rules/audit-checks.sh` advisory findings
this project currently carries, so `bad-rust-advisory-findings-unexplained`
(the reviewer-agent concern raised at commit `5fda62b`, receipt
`target/autobuilder/receipts/reviewer-agent.json`) does not recur verbatim.
Each entry below is deliberate and known, not an oversight; if the
underlying condition changes, update or remove the entry rather than
letting it go stale.

## HLT-016-SUPPLY-CHAIN-DRIFT (`cargo_deny_check`, `deny.toml`)

`deny.toml` runs `bans`/`licenses`/`sources` locally (in this audit pass)
but delegates the `advisories` check (RustSec vulnerability database) to
CI, where the vendored advisory DB is refreshed on every run instead of
whatever snapshot happens to be cached on a build box. Running
`advisories` locally against a stale local DB risks both false negatives
(a newly disclosed CVE the local snapshot predates) and false positives
(an advisory already resolved upstream), so it's deliberately left to the
CI job that always fetches fresh. This is a delegation, not a gap: the
check runs on every PR before merge.

## HLT-aux-PROPTEST-DENSITY (`check_proptest_density`, `tests/`)

The detector counts `proptest!()` invocations under `tests/` against
`pub fn` declarations under `src/` and flags anything below a 1-per-10
ceiling-divided threshold. PRD-mcphost-classify-precision added property
tests for `classify_stderr` (an in-crate `proptest!` block in
`src/sandbox.rs`'s `classify_proptests` module, since `classify_stderr` is
a private function integration tests cannot reach) and for `is_within`
(both an in-crate copy alongside `classify_stderr`'s tests, for locality,
and a `tests/classify_ac9_is_within_proptest.rs` integration test, since
`is_within` is public and that's what moves this specific tests/-scoped
metric). That takes the tests/-visible proptest count from 1 to 2 against
71 `pub fn`s (threshold 8) — real, deliberate movement toward the
threshold, not yet at it. Closing the remaining gap requires property
tests against other public functions across the crate, which is out of
this PRD's scope (classification-precision and hygiene only, per its
non-goals); a follow-on PRD is the right vehicle for the rest.

## HLT-041-COMMENT-HYGIENE (`grep_dangerous_comments`, `src/compat_check.rs:48`)

This finding was a false positive, not a real dangerous-comment marker:
the detector's `TEMP[: ]` pattern (meant to catch `TEMP:`/`TEMP ` marking
a known-temporary hack) matched case-insensitively against the ordinary
phrase "OS temp dir" in `ScratchDir`'s doc comment — coincidental
substring overlap, not an admission of a hack. PRD-mcphost-classify-precision
resolved it by rewording the comment (`std::env::temp_dir()` instead of
"OS temp dir", and stating the actual contract — this scratch directory
lives in `src/` because `mcphost migrate --check-compat` runs it from the
production CLI binary, not only under `cargo test`) so the marker no
longer fires while keeping the comment's meaning intact. Re-running
`scripts/audit.sh` after that change confirms no HLT-041 finding remains
for this file.
