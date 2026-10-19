//! contract: the core jobs' caps and the shard cache key are pinned in ci.yml
//! PRD-mcphost-ci-core-suites-sharded AC5 -- the static job is capped at <= 20
//! minutes and each core shard at <= 30 (10-18: the contracts family grew with
//! the registry axis; one shard ran 20 min 45 s and was cancelled at 20), the
//! aggregator at <= 5, no job except
//! determinism exceeds 30 (so PR #403's 45 is gone), and the shard job's
//! setup-rust-toolchain step sets a cache-key containing `matrix.shard`.

use crate::ci_sandbox_support;
use ci_sandbox_support as support;

fn timeout_of(body: &str) -> Option<u32> {
    body.lines()
        .find_map(|l| l.trim().strip_prefix("timeout-minutes:"))
        .and_then(|v| v.trim().parse().ok())
}

fn jobs() -> Vec<(String, String)> {
    support::jobs(&support::workflow())
}

fn body_of(name: &str) -> String {
    jobs().into_iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("job {name}")).1
}

#[test]
fn static_job_capped_at_20_core_shards_at_30_and_the_aggregator_at_5() {
    assert!(timeout_of(&body_of("core-static")).is_some_and(|t| t <= 20), "core-static");
    assert!(timeout_of(&body_of("core")).is_some_and(|t| t <= 30), "core shard");
    assert!(timeout_of(&body_of("gate")).is_some_and(|t| t <= 5), "aggregator");
}

#[test]
fn no_job_except_determinism_exceeds_30_minutes() {
    for (name, body) in jobs() {
        let Some(t) = timeout_of(&body) else { continue };
        if name == "determinism" {
            continue;
        }
        assert!(t <= 30, "job `{name}` has timeout-minutes {t} (> 30)");
    }
    assert!(
        !support::workflow().contains("timeout-minutes: 45"),
        "PR #403's 45-minute stopgap must be gone"
    );
}

#[test]
fn the_shard_toolchain_step_keys_its_cache_by_shard() {
    let body = body_of("core");
    let at = body
        .find("uses: actions-rust-lang/setup-rust-toolchain@")
        .expect("the shard job installs the toolchain with setup-rust-toolchain");
    let step: String = body[at..]
        .lines()
        .skip(1)
        .take_while(|l| !l.trim_start().starts_with("- name:") && !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let key = step
        .lines()
        .find_map(|l| l.trim().strip_prefix("cache-key:"))
        .unwrap_or_else(|| panic!("the toolchain step sets no cache-key:\n{step}"));
    assert!(key.contains("matrix.shard"), "cache-key must contain matrix.shard: {key}");
}
