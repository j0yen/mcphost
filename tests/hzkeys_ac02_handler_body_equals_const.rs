//! PRD-mcphost-healthz-anon-keys-const AC2 (P0) -- the handler's anonymous
//! body is built by `http::anon_healthz_body`; `hzkeys_ac02` is the one test
//! that asserts the served keys equal `ANON_HEALTHZ_KEYS` and names the const
//! when they differ. The fixture for "the handler's anonymous branch inserts
//! one extra key" is `AppState::anon_healthz_extra_key`, armed on this test's
//! own server when `HZKEYS_AC02_EXTRA_KEY` is set; the last test re-runs this
//! binary with it set and asserts exactly that one test fails.

use mcphost::build_info::{ANON_HEALTHZ_KEYS, BUILD};
use crate::common::TestServer;
use mcphost::http::{anon_healthz_body, check_anon_healthz_keys};

/// Set in the child run of [`an_extra_key_in_the_handler_fails_exactly_hzkeys_ac02`].
const EXTRA_KEY_ENV: &str = "HZKEYS_AC02_EXTRA_KEY";
const THIS_MODULE: &str = "hzkeys_ac02_handler_body_equals_const";
const LIVE_TEST: &str = "live_anonymous_healthz_keys_equal_anon_healthz_keys";
const META_TEST: &str = "an_extra_key_in_the_handler_fails_exactly_hzkeys_ac02";

#[test]
fn anonymous_handler_body_keys_equal_anon_healthz_keys() {
    for ok in [true, false] {
        let body = anon_healthz_body(ok, &BUILD);
        if let Err(msg) = check_anon_healthz_keys(&body) {
            panic!("{msg}");
        }
        let mut keys: Vec<&str> = body.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ANON_HEALTHZ_KEYS, "ANON_HEALTHZ_KEYS vs handler body (ok={ok})");
    }
}

#[tokio::test]
async fn live_anonymous_healthz_keys_equal_anon_healthz_keys() {
    let server = TestServer::start().await;
    if let Ok(extra) = std::env::var(EXTRA_KEY_ENV) {
        server.state.anon_healthz_extra_key.set(extra.leak()).expect("arm once");
    }
    let body: serde_json::Value = reqwest::get(format!("{}/healthz", server.base_url))
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse /healthz");
    if let Err(msg) = check_anon_healthz_keys(body.as_object().expect("object")) {
        panic!("{msg}");
    }
}

/// Every module under `tests/` that reads the anonymous key set: the tests a
/// changed anonymous body could break.
fn anon_body_readers(tests_dir: &std::path::Path) -> Vec<String> {
    let mut stems: Vec<String> = std::fs::read_dir(tests_dir)
        .expect("read tests/")
        .filter_map(|e| {
            let path = e.expect("dir entry").path();
            let stem = path.file_stem()?.to_str()?.to_string();
            let text = std::fs::read_to_string(&path).ok()?;
            let reads = ["anon_healthz_keys()", "assert_anon_healthz(", "check_anon_healthz_keys(", "ANON_HEALTHZ_KEYS"]
                .iter()
                .any(|needle| text.contains(needle));
            (path.extension()? == "rs" && !stem.starts_with("suite_") && reads).then_some(stem)
        })
        .collect();
    stems.sort();
    stems
}

#[test]
fn an_extra_key_in_the_handler_fails_exactly_hzkeys_ac02() {
    if std::env::var_os(EXTRA_KEY_ENV).is_some() {
        return; // the child run; never recurse
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let suite = std::fs::read_to_string(root.join(format!("tests/{}.rs", env!("CARGO_CRATE_NAME"))))
        .expect("read this suite's mod list");
    let readers = anon_body_readers(&root.join("tests"));
    assert!(readers.len() >= 8, "expected the healthz key-set readers, found {readers:?}");
    for stem in &readers {
        assert!(
            suite.contains(&format!("mod {stem};")),
            "{stem} reads the anonymous key set but is not in {}; the child run would not cover it",
            env!("CARGO_CRATE_NAME")
        );
    }

    let out = std::process::Command::new(std::env::current_exe().expect("current_exe"))
        .args(&readers)
        .args(["--skip", META_TEST])
        .env(EXTRA_KEY_ENV, "uptime_s")
        .output()
        .expect("re-run this test binary with the extra key armed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let result = |verdict: &str| -> Vec<String> {
        stdout
            .lines()
            .filter_map(|l| l.strip_prefix("test ")?.strip_suffix(&format!(" ... {verdict}")).map(str::to_string))
            .collect()
    };
    let (failed, passed) = (result("FAILED"), result("ok"));

    assert_eq!(failed, vec![format!("{THIS_MODULE}::{LIVE_TEST}")], "exactly hzkeys_ac02 must fail:\n{stdout}");
    assert!(stdout.contains("differ from ANON_HEALTHZ_KEYS"), "the failure must name the const:\n{stdout}");
    assert!(stdout.contains("uptime_s"), "the failure must name the stray key:\n{stdout}");
    for stem in readers.iter().filter(|s| s.as_str() != THIS_MODULE) {
        assert!(
            passed.iter().any(|t| t.starts_with(&format!("{stem}::"))),
            "{stem} must run and pass in the child:\n{stdout}"
        );
    }
}
