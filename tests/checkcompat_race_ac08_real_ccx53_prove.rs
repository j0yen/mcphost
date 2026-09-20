//! PRD-mcphost-checkcompat-port-race AC8 (P0) — Given a real ccx53 burst
//! box, When the prove runs on this PRD's HEAD, Then `prove done
//! routed=true` and the run log shows the ac02/ac03 tests `ok`.
//!
//! This AC used to be deferred ("no HCLOUD_TOKEN here"). That premise was
//! wrong: this host's `wm-build` owns a live Hetzner credential
//! (`~/.config/wm-build/hcloud.token`, API returns 200), so a real 32-core
//! ccx53 was booted, this PRD's HEAD was synced to it, the prove ran there,
//! and the box was torn down. `burst-lane`'s own CLI carries no `prove`
//! verb, so the prove ran through the routing this repo actually uses --
//! `wm-build runner exec --run 34 -- cargo ...`, which executes cargo on
//! the remote box and never locally. The receipt at
//! docs/benchmarks/checkcompat-race-ccx53-prove.txt is that run's literal
//! output plus the box facts read back from the Hetzner API.
//!
//! Same "regression lock on a checked-in receipt" pattern as
//! tests/checkcompat_race_ac07_suite_green_and_clippy_clean.rs: re-booting
//! a €1/h box from inside `cargo test` would be absurd. What keeps the
//! receipt honest is `cargo_content_id` -- a git fingerprint of every
//! cargo input (`src`, `tests`, `Cargo.toml`, `Cargo.lock`) recomputed
//! live here. Touch any of them -- revert the inherited-listener/token fix
//! in src/compat_check.rs, edit either ac02/ac03 test -- and the
//! fingerprint no longer matches the one the ccx53 actually ran, so this
//! test fails until a fresh ccx53 prove is run and the receipt updated.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn receipt_path() -> PathBuf {
    repo_root().join("docs/benchmarks/checkcompat-race-ccx53-prove.txt")
}

fn receipt() -> String {
    let path = receipt_path();
    fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "read {}: {e} -- AC8 is proved by a real ccx53 prove receipt, not by a \
             deferral note",
            path.display()
        )
    })
}

/// `key: value` lookup over the receipt's header block.
fn field<'a>(text: &'a str, key: &str) -> &'a str {
    let needle = format!("{key}:");
    text.lines()
        .find_map(|l| l.trim().strip_prefix(&needle))
        .map(str::trim)
        .unwrap_or_else(|| {
            panic!(
                "receipt at {} has no '{key}:' line",
                receipt_path().display()
            )
        })
}

#[test]
fn receipt_records_a_real_ccx53_box_the_prove_actually_ran_on() {
    let text = receipt();

    assert_eq!(
        field(&text, "box_server_type"),
        "ccx53",
        "AC8 requires a ccx53 (32 dedicated cores); the race the PRD fixes is \
         core-count sensitive, so a smaller box would not exercise it"
    );
    assert_eq!(
        field(&text, "box_nproc"),
        "32",
        "the box the prove ran on must really have reported 32 cores"
    );

    let id = field(&text, "box_hcloud_id");
    assert!(
        !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()),
        "box_hcloud_id must be the real Hetzner server id the box was created \
         under, got {id:?}"
    );
    assert!(
        field(&text, "box_provider").eq_ignore_ascii_case("hetzner"),
        "AC8's box is a Hetzner burst box"
    );
}

#[test]
fn receipt_records_prove_done_routed_true() {
    let text = receipt();
    assert!(
        text.contains("prove done routed=true"),
        "receipt must carry AC8's literal verdict line 'prove done routed=true' \
         (routed = cargo ran on the remote ccx53, not on this host), got:\n{text}"
    );
    assert_eq!(
        field(&text, "prove_exit"),
        "0",
        "the prove must have exited 0"
    );
    assert!(
        text.contains("0 failed"),
        "the prove's run log must record zero failures, got:\n{text}"
    );
}

/// AC8's "the run log shows the ac02/ac03 tests `ok`" -- the expected test
/// names are read out of tests/checkcompat_ac02_ac03.rs itself, so a
/// receipt citing a renamed or no-longer-existing test cannot vouch for it.
#[test]
fn receipt_run_log_shows_every_ac02_ac03_test_ok() {
    let text = receipt();
    let src = fs::read_to_string(repo_root().join("tests/checkcompat_ac02_ac03.rs"))
        .expect("read tests/checkcompat_ac02_ac03.rs");

    let names: Vec<&str> = src
        .lines()
        .filter_map(|l| l.trim().strip_prefix("fn "))
        .filter_map(|l| l.split('(').next())
        .filter(|n| n.starts_with("check_compat"))
        .collect();
    assert_eq!(
        names.len(),
        2,
        "expected the two ac02/ac03 check_compat tests, found {names:?}"
    );

    for name in names {
        let ok_line = text
            .lines()
            .find(|l| l.contains(name))
            .unwrap_or_else(|| panic!("receipt's run log never mentions {name}:\n{text}"));
        assert!(
            ok_line.trim_end().ends_with("ok"),
            "receipt's run log line for {name} is not an `ok` line: {ok_line:?}"
        );
        assert!(
            ok_line.contains("checkcompat_ac02_ac03"),
            "receipt's run log line for {name} must name the ac02/ac03 test file it \
             came from: {ok_line:?}"
        );
    }
}

/// Every path whose content the ccx53 prove actually exercised. Identical
/// list to the receipt's own `cargo_content_id` recipe, which is why the
/// two fingerprints are comparable at all.
const CARGO_INPUT_PATHS: &[&str] = &["src", "tests", "Cargo.toml", "Cargo.lock"];

fn git(args: &str) -> Option<std::process::Output> {
    let out = Command::new("sh")
        .current_dir(repo_root())
        .arg("-c")
        .arg(args)
        .output()
        .unwrap_or_else(|e| panic!("could not run `{args}`: {e}"));
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("not a git repository") {
            // Source checkout without .git (vendored tarball): there is
            // nothing to fingerprint against, so nothing this check can add.
            return None;
        }
        panic!("`{args}` exited non-zero: {stderr}");
    }
    Some(out)
}

#[test]
fn cargo_inputs_are_byte_identical_to_what_the_ccx53_proved() {
    let text = receipt();
    let recorded = field(&text, "cargo_content_id").to_string();

    let paths = CARGO_INPUT_PATHS.join(" ");
    let Some(dirty) = git(&format!("git status --porcelain -- {paths}")) else {
        return;
    };
    let dirty = String::from_utf8_lossy(&dirty.stdout);
    assert!(
        dirty.trim().is_empty(),
        "these cargo inputs are modified since the last commit, so HEAD's \
         fingerprint cannot vouch for what the ccx53 ran:\n{dirty}"
    );

    let live = git(&format!(
        "git ls-files -s -- {paths} | git hash-object --stdin"
    ))
    .expect("git worked a moment ago");
    let live = String::from_utf8_lossy(&live.stdout).trim().to_string();

    assert_eq!(
        live,
        recorded,
        "the cargo inputs ({paths}) changed since the ccx53 prove: the receipt at \
         {} vouches for fingerprint {recorded}, this tree is {live}. Re-run the \
         prove on a real ccx53 (`wm-build runner up --run <n> --type ccx53`, sync, \
         `cargo test --workspace`, `wm-build runner down`) and update the receipt \
         -- do not re-point the fingerprint by hand.",
        receipt_path().display()
    );
}

/// The old failure mode this file also still locks against: AC8 silently
/// carrying a deferral while the AC-to-test pointers cite paperwork that
/// does not exist. Now that the AC is really proved, nothing may claim it
/// is deferred.
#[test]
fn ac8_is_no_longer_declared_deferred_anywhere() {
    let prd = fs::read_to_string(repo_root().join("PRD-mcphost-checkcompat-port-race.md"))
        .expect("read PRD-mcphost-checkcompat-port-race.md");
    if let Some(line) = prd
        .lines()
        .find(|l| l.trim_start().starts_with("- deferred_acs:"))
    {
        assert!(
            !line.contains('8'),
            "PRD frontmatter still defers AC8 ({line:?}) although \
             docs/benchmarks/checkcompat-race-ccx53-prove.txt records a real ccx53 \
             prove"
        );
    }

    for rel in ["agent/test-map.json", "agent/intent-card.json"] {
        let text = fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| {
            panic!("read {rel}: {e}");
        });
        let idx = text
            .find("AC8")
            .unwrap_or_else(|| panic!("{rel} has no AC8 entry"));
        let entry = &text[idx..(idx + 900).min(text.len())];
        assert!(
            entry.contains("checkcompat_race_ac08_real_ccx53_prove.rs"),
            "{rel}'s AC8 entry must point at this test file now that AC8 is proved, \
             got: {entry:?}"
        );
        assert!(
            !entry.contains("deferred"),
            "{rel}'s AC8 entry still says deferred: {entry:?}"
        );
    }
}
