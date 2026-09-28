//! PRD-mcphost-docs-qa-recipe
//! AC10 (P0, Live) -- Given the truth-tier panel run after land, When
//! `synthorg consume` runs with the pinned panel, Then `rag_indexer`
//! satisfaction is reported with the new task included (number recorded in
//! the vision, target >= 75%).
//!
//! **AC10's Then is DEFERRED -- operator-provisioned, and nothing in this
//! file is counted as its proof.** The number can only come from a
//! truth-tier `synthorg consume --tier truth --endpoint
//! https://mcphost.dev/mcp` run: ~$5 and ~40 min of real frontier calls,
//! billed to the operator's Anthropic key (`ANTHROPIC_API_KEY` /
//! `WM_ANTHROPIC_API_KEY`, which this sandbox does not hold -- `synthorg.llm`
//! refuses live mode without it), driven from `~/repos/synthorg` (a different
//! repository, python, and not present on this gate's runner box) by a
//! RedBaron user timer on a live host, against a prod deployment that does
//! not carry this branch until the operator deploys it. The deferral is
//! declared in the PRD's own frontmatter (`deferred_acs: [9, 10]` plus a
//! `mock_justifications` entry naming this file and the live test fn) and
//! locked by `tests/docsqa_ac10_deferral_is_justified.rs`, so it is
//! machine-visible rather than a promise. The one test that can ever claim
//! AC10's Then is `live_truth_tier_panel_run_meets_the_target` below, which
//! fails once `MCPHOST_LIVE=1` until that run's measure.json really reports
//! satisfaction[rag_indexer] >= 75% with `docs_qa_recipe` in the corpus:
//!
//!   MCPHOST_LIVE=1 [MCPHOST_PANEL_MEASURE=<run>/measure.json] \
//!     cargo test --test suite_core_NN docsqa_ac10
//!
//! Env vars, read only when `MCPHOST_LIVE=1`:
//!   MCPHOST_PANEL_MEASURE  the post-land truth-tier run's measure.json
//!                          (default: the newest
//!                          ~/repos/synthorg/runs/truth-tier-*/measure.json)
//!   MCPHOST_VISION_DOC     vision doc to record into (default:
//!                          ~/Documents/PRDs/visions/mcphost-data-layer-first-slice.md)
//!
//! What the always-on tests below own is the branch's own mechanism -- the
//! whole path between "that run finished" and "the number is recorded",
//! `examples/docs-qa/record-panel-satisfaction.sh`, run as a real child
//! process on every `cargo test`:
//!
//! * **The inclusion check.** AC10 says "with the new task included". The
//!   recorder refuses (exit 2, writing nothing) any run whose
//!   `corpus.task_ids` lacks `docs_qa_recipe`, and the fixture it refuses is
//!   a *real* truth-tier nightly (`runs/truth-tier-20260926T025631Z`,
//!   `rag_indexer` 62.5%, 28 tasks) -- so a genuine, expensive, live panel
//!   number still does not answer this AC if the task was not in the corpus.
//! * **The truth-vs-offline split.** An offline full-corpus run over the same
//!   pinned composition (`SYNTHORG_LLM_MODE=fake`: stub personas, stub judge,
//!   in-process fake endpoint) was really performed for this build --
//!   `runs/offline-docsqa-20260926T053158Z`, `rag_indexer` 90.0% with
//!   `docs_qa_recipe` included -- and the recorder labels it `offline` and
//!   refuses to call it green, so it can never be mistaken for the market
//!   number the PRD's success metric names.
//! * **The target comparison and the record.** The exact dated bullet, in the
//!   vision doc's `## Satisfaction record` section and in this repo's own
//!   `docs/receipts/docs-qa-panel-satisfaction.md`, idempotent per run
//!   directory, with the number recorded even when it misses the target
//!   (AC10 asks for the number; a miss is a number too).
//!
//! No number in this file is invented. Both fixtures are faithful trims of
//! real `measure.json` files (byte digests pinned in the receipt), and the
//! measures the target comparison is driven with are *assembled* from those
//! two real runs -- the truth-tier nightly's own tier/client/endpoint
//! identity, with the offline run's own corpus (the one that really contains
//! `docs_qa_recipe`) and its own satisfaction and session counts, or the
//! truth nightly's own under-target number. Nothing is hand-set, spliced or
//! rounded, and `the_measures_under_test_invent_no_number` fails if a later
//! edit tries to.

use crate::docs_qa;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const SEGMENT: &str = "rag_indexer";
const TASK: &str = "docs_qa_recipe";
const RECEIPT: &str = "docs/receipts/docs-qa-panel-satisfaction.md";
const REAL_TRUTH_FIXTURE: &str = "panel-measure-truth-20260926T025631Z.json";
const REAL_OFFLINE_FIXTURE: &str = "panel-measure-offline-20260926T053158Z.json";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn recorder() -> PathBuf {
    docs_qa::examples_dir().join("record-panel-satisfaction.sh")
}

fn fixture_path(name: &str) -> PathBuf {
    docs_qa::examples_dir().join("fixtures").join(name)
}

fn load_fixture(name: &str) -> Value {
    let path = fixture_path(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

/// The byte digest of a fixture as committed, so the receipt can pin each
/// fixture to the real run it was trimmed from and a later edit of a
/// *number* cannot pass unnoticed.
fn fixture_digest(name: &str) -> String {
    let bytes = std::fs::read(fixture_path(name)).unwrap_or_else(|e| panic!("read fixture {name}: {e}"));
    format!("{:x}", Sha256::digest(&bytes))
}

fn satisfaction_of(measure: &Value) -> f64 {
    measure["satisfaction"]["by_segment"][SEGMENT]
        .as_f64()
        .unwrap_or_else(|| panic!("satisfaction[{SEGMENT}] missing or not a number"))
}

/// How the recorder renders a fraction in its line, so every expectation in
/// this file is derived from the fixture rather than typed out.
fn percent(value: f64) -> String {
    format!("{:.1}%", value * 100.0)
}

struct Recorded {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Recorded {
    fn line(&self) -> String {
        self.stdout
            .lines()
            .find_map(|l| l.strip_prefix("LINE: "))
            .unwrap_or_else(|| panic!("recorder printed no LINE:\nstdout:\n{}\nstderr:\n{}", self.stdout, self.stderr))
            .to_string()
    }
}

/// The recorder's argv, as a pure function of the three paths -- the
/// always-on tests and the `MCPHOST_LIVE=1` half both build their invocation
/// through this, so the live half's command cannot drift from the proven one
/// (or from what the receipt documents for the operator).
fn recorder_args(measure: &str, vision: Option<&str>, receipt: Option<&str>) -> Vec<String> {
    let mut args = vec!["--measure".to_string(), measure.to_string()];
    if let Some(vision) = vision {
        args.push("--vision".to_string());
        args.push(vision.to_string());
    }
    if let Some(receipt) = receipt {
        args.push("--receipt".to_string());
        args.push(receipt.to_string());
    }
    args
}

/// The live half's verdict on the recorder's exit code, as a pure function:
/// only a live truth-tier run that met the target is AC10's green. Exit 1
/// (recorded but offline or under target) and exit 2 (refused) are both
/// failures of the AC, with different reasons.
fn live_verdict(code: i32) -> Result<(), String> {
    match code {
        0 => Ok(()),
        1 => Err(format!(
            "the run was recorded but is not AC10's green: either an offline fake-mode run or \
             satisfaction[{SEGMENT}] under the 75% target"
        )),
        2 => Err(format!(
            "the recorder refused the run: not a truth-tier panel run, or its corpus lacks {TASK}"
        )),
        other => Err(format!("the recorder failed unexpectedly (exit {other})")),
    }
}

fn run_recorder(args: &[String]) -> Recorded {
    let output = std::process::Command::new("bash")
        .arg(recorder())
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("run {}: {e}", recorder().display()));
    Recorded {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// Lays a measure.json out the way a synthorg run does -- `<root>/runs/<run
/// name>/measure.json` -- so the recorded line names the run the same
/// `runs/<run>` way the vision docs' other dated run bullets do.
fn stage_run(scratch: &Path, run_name: &str, measure: &Value) -> PathBuf {
    let dir = scratch.join("runs").join(run_name);
    std::fs::create_dir_all(&dir).expect("create staged run dir");
    let path = dir.join("measure.json");
    std::fs::write(&path, serde_json::to_string_pretty(measure).expect("serialize measure")).expect("write measure");
    path
}

/// A minimal stand-in for the vision doc: the recorder creates its own
/// `## Satisfaction record` section, so what matters is only that it does
/// not disturb the prose around it.
fn scratch_doc(scratch: &Path, name: &str) -> PathBuf {
    let path = scratch.join(name);
    std::fs::write(&path, "# vision\n\n## Loop contract\n\nBaseline: 61.8% for the persona (2026-09-23).\n")
        .expect("write scratch doc");
    path
}

/// The two real runs, assembled: the truth-tier nightly's own identity
/// (`tier`, `client`, `client_version`, `endpoint_version`, judge/scorer
/// versions, the pinned composition) carrying the run whose numbers come
/// from `measured` -- its whole `corpus` object (fingerprint included, so the
/// corpus is not a mutated array), its `satisfaction`, and its per-segment
/// session counts. Every value is one of the two runs' own; nothing is
/// hand-set. `measured = REAL_OFFLINE_FIXTURE` is the shape of the post-land
/// run AC10 waits for (task included, 90.0% >= target); `measured =
/// REAL_TRUTH_FIXTURE`'s numbers over the offline corpus is the under-target
/// case (62.5%).
fn measure_from_real_runs(measured: &str) -> Value {
    let mut measure = load_fixture(REAL_TRUTH_FIXTURE);
    let source = load_fixture(measured);
    let numbers = load_fixture(REAL_OFFLINE_FIXTURE);
    // The corpus always comes from the offline run: it is the only real run
    // whose corpus actually contains this recipe's task.
    measure["corpus"] = numbers["corpus"].clone();
    measure["satisfaction"] = source["satisfaction"].clone();
    measure["scored_by_segment"] = source["scored_by_segment"].clone();
    measure["sessions_by_segment"] = source["sessions_by_segment"].clone();
    measure
}

fn live_mode_enabled(raw: Option<&str>) -> bool {
    raw == Some("1")
}

/// Which measure.json the `MCPHOST_LIVE=1` half drives. A pure function of
/// its argument so the default is asserted without reading (or mutating)
/// the ambient environment.
fn resolve_live_measure(env_override: Option<&str>) -> String {
    match env_override {
        Some(path) if !path.is_empty() => path.to_string(),
        _ => {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
            let mut runs: Vec<PathBuf> = glob_truth_tier_runs(Path::new(&home).join("repos/synthorg/runs"));
            runs.sort();
            match runs.pop() {
                Some(dir) => dir.join("measure.json").display().to_string(),
                None => String::new(),
            }
        }
    }
}

fn glob_truth_tier_runs(runs_dir: PathBuf) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(&runs_dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("truth-tier-"))
                && p.join("measure.json").is_file()
        })
        .collect()
}

fn default_vision_doc() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    Path::new(&home).join("Documents/PRDs/visions/mcphost-data-layer-first-slice.md")
}

/// Provenance, asserted before anything is driven with it: the measures the
/// target comparison runs on carry only values the two real runs really
/// reported. This is what keeps the mechanism proof honest -- it is a real
/// pair of panel runs re-assembled, not a hand-written number that happens
/// to be over 75%.
#[test]
fn the_measures_under_test_invent_no_number() {
    let truth = load_fixture(REAL_TRUTH_FIXTURE);
    let offline = load_fixture(REAL_OFFLINE_FIXTURE);

    // The offline run is the only one whose corpus really has the task; the
    // truth nightly predates it. Neither array is edited anywhere here.
    assert!(
        offline["corpus"]["task_ids"]
            .as_array()
            .expect("offline task_ids")
            .iter()
            .any(|t| t == TASK),
        "the offline run must really include {TASK}"
    );
    assert!(
        !truth["corpus"]["task_ids"]
            .as_array()
            .expect("truth task_ids")
            .iter()
            .any(|t| t == TASK),
        "the truth nightly must be a run from before the task existed"
    );

    for (measured, label) in [(REAL_OFFLINE_FIXTURE, "post-land shape"), (REAL_TRUTH_FIXTURE, "under target")] {
        let assembled = measure_from_real_runs(measured);
        let source = load_fixture(measured);
        for key in ["tier", "client", "client_version", "endpoint_version", "judge_version", "scorer_version"] {
            assert_eq!(assembled[key], truth[key], "{label}: {key} must be the truth nightly's own value");
        }
        assert_eq!(
            assembled["composition"], truth["composition"],
            "{label}: the pinned panel composition must be the real run's"
        );
        assert_eq!(assembled["corpus"], offline["corpus"], "{label}: the corpus must be the offline run's own");
        assert_eq!(
            assembled["satisfaction"], source["satisfaction"],
            "{label}: the number must be {measured}'s own, not a hand-set one"
        );
        assert_eq!(assembled["scored_by_segment"], source["scored_by_segment"], "{label}: scoring counts");
        assert_eq!(assembled["sessions_by_segment"], source["sessions_by_segment"], "{label}: session counts");
    }

    // And the two really do straddle the target, so the green/not-green
    // pair below is decided by real measurements.
    assert!(satisfaction_of(&offline) >= 0.75, "offline run: {}", satisfaction_of(&offline));
    assert!(satisfaction_of(&truth) < 0.75, "truth nightly: {}", satisfaction_of(&truth));
}

/// The recorder's green path -- NOT a proof of AC10's Then: no `synthorg
/// consume` run against the pinned panel happens here, truth-tier or
/// otherwise (see this file's header; the run is operator-provisioned). What
/// it proves is that when a post-land truth-tier measure does arrive, the
/// recorder writes the number into both documents, marks it green against
/// the 75% target, and stays idempotent on replay. The measure it is driven
/// with is assembled only from the two real runs' own values
/// (`the_measures_under_test_invent_no_number`).
#[test]
fn the_recorder_marks_a_measure_assembled_from_real_runs_green_and_writes_the_line() {
    let scratch = docs_qa::scratch_receipt_dir("ac10-postland");
    let measure_json = measure_from_real_runs(REAL_OFFLINE_FIXTURE);
    let expected = percent(satisfaction_of(&measure_json));
    let measure = stage_run(&scratch, "truth-tier-20260927T000000Z", &measure_json);
    let vision = scratch_doc(&scratch, "vision.md");
    let receipt = scratch_doc(&scratch, "receipt.md");

    let run = run_recorder(&recorder_args(
        &measure.display().to_string(),
        Some(&vision.display().to_string()),
        Some(&receipt.display().to_string()),
    ));
    assert_eq!(
        run.code, 0,
        "a truth-tier run with {TASK} at {expected} must be green\nstdout:\n{}\nstderr:\n{}",
        run.stdout, run.stderr
    );
    assert_eq!(live_verdict(run.code), Ok(()), "the live half's own verdict fn must call this green");
    assert!(run.stdout.contains("TIER: truth-tier"), "stdout:\n{}", run.stdout);
    assert!(run.stdout.contains("GREEN:"), "stdout:\n{}", run.stdout);

    let line = run.line();
    for needle in [
        "truth-tier run runs/truth-tier-20260927T000000Z".to_string(),
        format!("satisfaction[{SEGMENT}]={expected}"),
        "target >= 75%".to_string(),
        format!("{TASK} included"),
    ] {
        assert!(line.contains(&needle), "recorded line must name {needle:?}: {line}");
    }

    for doc in [&vision, &receipt] {
        let text = std::fs::read_to_string(doc).expect("read recorded doc");
        assert!(
            text.contains("## Satisfaction record — satisfaction[rag_indexer]"),
            "{}: {text}",
            doc.display()
        );
        assert!(text.contains(&line), "{} must carry the recorded line: {text}", doc.display());
        assert!(
            text.contains("## Loop contract"),
            "{}: the doc's existing prose must survive: {text}",
            doc.display()
        );
    }

    // The nightly can hand the same run to the recorder more than once (a
    // retried lift step, a re-run of the cross-repo commit); the record is
    // a log, not a pile of duplicates.
    let again = run_recorder(&recorder_args(
        &measure.display().to_string(),
        Some(&vision.display().to_string()),
        None,
    ));
    assert!(again.stdout.contains("ALREADY-RECORDED:"), "stdout:\n{}", again.stdout);
    let text = std::fs::read_to_string(&vision).expect("read vision");
    assert_eq!(
        text.matches("run runs/truth-tier-20260927T000000Z:").count(),
        1,
        "the same run must not be recorded twice: {text}"
    );
}

/// The clause that makes AC10 more than "some panel number exists": a real,
/// live, expensive truth-tier run whose corpus did not include this
/// recipe's task is refused outright, and nothing is written.
#[test]
fn a_panel_run_without_the_new_task_is_refused_and_records_nothing() {
    let scratch = docs_qa::scratch_receipt_dir("ac10-without-task");
    let measure = fixture_path(REAL_TRUTH_FIXTURE);
    let vision = scratch_doc(&scratch, "vision.md");
    let before = std::fs::read_to_string(&vision).expect("read vision");

    // The fixture really is a truth-tier run that really does report the
    // segment -- the refusal is about the corpus, nothing else.
    let fixture = load_fixture(REAL_TRUTH_FIXTURE);
    assert_eq!(fixture["tier"], "truth");
    assert!(fixture["satisfaction"]["by_segment"][SEGMENT].is_number());

    let run = run_recorder(&recorder_args(
        &measure.display().to_string(),
        Some(&vision.display().to_string()),
        None,
    ));
    assert_eq!(run.code, 2, "stdout:\n{}\nstderr:\n{}", run.stdout, run.stderr);
    assert!(
        run.stderr.contains("REFUSED:") && run.stderr.contains(TASK),
        "the refusal must name the missing task: {}",
        run.stderr
    );
    assert!(live_verdict(run.code).is_err(), "a refused run can never be AC10's green");
    assert_eq!(
        std::fs::read_to_string(&vision).expect("read vision"),
        before,
        "a refused run must write nothing"
    );
}

/// The number is the deliverable, not the target: a miss is recorded too,
/// just not green. The miss is the truth nightly's own 62.5%, over the
/// offline run's corpus -- again a real measurement, not a chosen one.
#[test]
fn a_number_under_the_target_is_recorded_but_not_green() {
    let scratch = docs_qa::scratch_receipt_dir("ac10-under-target");
    let measure_json = measure_from_real_runs(REAL_TRUTH_FIXTURE);
    let expected = percent(satisfaction_of(&measure_json));
    let measure = stage_run(&scratch, "truth-tier-20260927T010000Z", &measure_json);
    let vision = scratch_doc(&scratch, "vision.md");

    let run = run_recorder(&recorder_args(
        &measure.display().to_string(),
        Some(&vision.display().to_string()),
        None,
    ));
    assert_eq!(run.code, 1, "stdout:\n{}\nstderr:\n{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("NOT-GREEN:"), "stdout:\n{}", run.stdout);
    assert!(live_verdict(run.code).is_err(), "an under-target number is not AC10's green");
    let text = std::fs::read_to_string(&vision).expect("read vision");
    assert!(
        text.contains(&format!("satisfaction[{SEGMENT}]={expected}")),
        "an under-target number ({expected}) is still recorded: {text}"
    );
}

/// The offline run this build really did perform proves the task is wired
/// into the pinned panel -- and is structurally barred from standing in for
/// the truth-tier row.
#[test]
fn the_offline_fake_mode_run_is_recorded_as_offline_not_as_the_truth_tier_row() {
    let scratch = docs_qa::scratch_receipt_dir("ac10-offline");
    let fixture = load_fixture(REAL_OFFLINE_FIXTURE);
    assert!(
        fixture["corpus"]["task_ids"]
            .as_array()
            .expect("task_ids")
            .iter()
            .any(|t| t == TASK),
        "the offline run must include {TASK}"
    );
    let composition = fixture["composition"]["source"].as_str().expect("composition.source");
    assert!(
        composition.ends_with("corpora/mcphost/panel-composition.yaml"),
        "the offline run must use the pinned panel composition, got {composition:?}"
    );

    let measure = stage_run(&scratch, "offline-docsqa-20260926T053158Z", &fixture);
    let vision = scratch_doc(&scratch, "vision.md");
    let run = run_recorder(&recorder_args(
        &measure.display().to_string(),
        Some(&vision.display().to_string()),
        None,
    ));
    assert_eq!(run.code, 1, "stdout:\n{}\nstderr:\n{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("TIER: offline"), "stdout:\n{}", run.stdout);
    assert!(
        run.stdout.contains("NOT-GREEN:") && run.stdout.contains("cannot stand in for the truth-tier"),
        "stdout:\n{}",
        run.stdout
    );
    assert!(
        live_verdict(run.code).is_err(),
        "the offline run this build performed must never satisfy AC10's live half"
    );
    let line = run.line();
    assert!(line.contains("offline run runs/offline-docsqa-"), "{line}");
    assert!(!line.contains("truth-tier run"), "an offline row must not read as truth-tier: {line}");
}

/// Nothing but a truth-tier panel run can be recorded at all -- a proxy-tier
/// regression run is a different measurement.
#[test]
fn a_non_truth_tier_run_is_refused() {
    let scratch = docs_qa::scratch_receipt_dir("ac10-proxy");
    let mut measure = measure_from_real_runs(REAL_OFFLINE_FIXTURE);
    measure["tier"] = json!("proxy");
    let staged = stage_run(&scratch, "proxy-20260927T020000Z", &measure);

    let run = run_recorder(&recorder_args(&staged.display().to_string(), None, None));
    assert_eq!(run.code, 2, "stdout:\n{}\nstderr:\n{}", run.stdout, run.stderr);
    assert!(run.stderr.contains("truth-tier"), "{}", run.stderr);
}

/// The committed record: this repo's own receipt carries the baseline, the
/// target, the offline row that was really measured, the exact post-land
/// command -- and cannot drift from the fixtures the rows came from.
#[test]
fn the_committed_receipt_records_the_baseline_the_offline_row_and_the_post_land_command() {
    let path = repo_root().join(RECEIPT);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

    for needle in [
        "61.8%",
        "≥ 75%",
        TASK,
        "## Satisfaction record — satisfaction[rag_indexer]",
        "visions/mcphost-data-layer-first-slice.md",
        "examples/docs-qa/record-panel-satisfaction.sh",
        "--tier truth",
        "corpora/mcphost/panel-composition.yaml",
        "MCPHOST_PANEL_MEASURE",
        "deferred_acs",
    ] {
        assert!(text.contains(needle), "{} must name {needle:?}", path.display());
    }

    let offline = load_fixture(REAL_OFFLINE_FIXTURE);
    let recorded = format!("satisfaction[rag_indexer]={}", percent(satisfaction_of(&offline)));
    assert!(
        text.contains(&recorded),
        "the receipt's offline row must match the fixture it came from ({recorded}): {text}"
    );
    assert!(
        text.contains("offline run runs/offline-docsqa-20260926T053158Z"),
        "the receipt must name the offline run it recorded: {text}"
    );
}

/// Both fixtures are trims of real `~/repos/synthorg` runs that no test on
/// this gate's box can re-read (synthorg is another repo, not present
/// there). What keeps them honest is the receipt: it pins each fixture's
/// byte digest next to the run directory it came from, so editing a number
/// in a fixture -- the one way a mechanism proof could quietly turn into an
/// invented one -- fails here.
#[test]
fn the_receipt_pins_each_fixture_to_the_real_run_it_was_trimmed_from() {
    let path = repo_root().join(RECEIPT);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

    for (fixture, run_dir) in [
        (REAL_TRUTH_FIXTURE, "runs/truth-tier-20260926T025631Z"),
        (REAL_OFFLINE_FIXTURE, "runs/offline-docsqa-20260926T053158Z"),
    ] {
        let digest = fixture_digest(fixture);
        assert!(
            text.contains(&digest),
            "the receipt must pin {fixture}'s sha256 ({digest}) -- it is the only check that the \
             numbers this file drives the recorder with are still the real run's"
        );
        assert!(text.contains(run_dir), "the receipt must name {run_dir}, {fixture}'s source run");
    }
}

/// The live half: the operator's post-land truth-tier run, recorded into the
/// real vision doc. Only `MCPHOST_LIVE=1` performs it; by default this
/// asserts the paths and argv it would drive, so the two halves cannot
/// drift. It fails until the operator's run has happened -- that is what
/// makes AC10's deferral machine-visible rather than a promise.
#[test]
fn live_truth_tier_panel_run_meets_the_target() {
    assert_eq!(
        resolve_live_measure(Some("/tmp/a-run/measure.json")),
        "/tmp/a-run/measure.json",
        "MCPHOST_PANEL_MEASURE must win when set"
    );
    assert!(
        default_vision_doc().ends_with("Documents/PRDs/visions/mcphost-data-layer-first-slice.md"),
        "default vision doc: {}",
        default_vision_doc().display()
    );

    if !live_mode_enabled(std::env::var("MCPHOST_LIVE").ok().as_deref()) {
        println!(
            "AC10 live half skipped (MCPHOST_LIVE unset): the truth-tier panel run is \
             operator-authorized and paid (deferred -- operator-provisioned, see this file's \
             header and the PRD's mock_justifications); default measure would be {:?}",
            resolve_live_measure(None)
        );
        return;
    }

    let measure = resolve_live_measure(std::env::var("MCPHOST_PANEL_MEASURE").ok().as_deref());
    assert!(
        !measure.is_empty(),
        "no truth-tier run found: set MCPHOST_PANEL_MEASURE to the post-land run's measure.json"
    );
    let vision = std::env::var("MCPHOST_VISION_DOC")
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_vision_doc());
    let receipt = repo_root().join(RECEIPT);

    let run = run_recorder(&recorder_args(
        &measure,
        Some(&vision.display().to_string()),
        Some(&receipt.display().to_string()),
    ));
    println!("AC10 live proof ({measure}):\n{}{}", run.stdout, run.stderr);
    if let Err(why) = live_verdict(run.code) {
        panic!(
            "the post-land truth-tier run must report satisfaction[{SEGMENT}] >= 75% with {TASK} \
             included -- {why}\nstdout:\n{}\nstderr:\n{}",
            run.stdout, run.stderr
        );
    }
}

/// The live half's own decisions, exercised on every run so only the paid
/// panel run itself is unproven: which argv it builds, and which exit codes
/// it is allowed to call green.
#[test]
fn the_live_half_builds_the_documented_recorder_argv() {
    assert_eq!(
        recorder_args("/runs/r/measure.json", Some("/v.md"), Some("/r.md")),
        vec!["--measure", "/runs/r/measure.json", "--vision", "/v.md", "--receipt", "/r.md"]
    );
    assert_eq!(
        recorder_args("/runs/r/measure.json", None, None),
        vec!["--measure", "/runs/r/measure.json"],
        "with neither document the recorder only reports"
    );

    // The receipt documents this exact command for the operator's post-land
    // run; the live half must not invent a different one.
    let path = repo_root().join(RECEIPT);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    for flag in recorder_args("x", Some("y"), Some("z")).into_iter().filter(|a| a.starts_with("--")) {
        assert!(text.contains(&flag), "the receipt's post-land command must pass {flag}: {text}");
    }
}

#[test]
fn live_verdict_is_green_only_on_exit_zero() {
    assert_eq!(live_verdict(0), Ok(()));
    assert!(live_verdict(1).expect_err("exit 1 is not green").contains("under the 75% target"));
    assert!(live_verdict(2).expect_err("exit 2 is not green").contains("refused"));
    assert!(live_verdict(-1).expect_err("a crash is not green").contains("unexpectedly"));
}

#[test]
fn live_mode_is_disabled_when_mcphost_live_is_unset_or_not_1() {
    assert!(!live_mode_enabled(None), "unset must disable live mode");
    assert!(!live_mode_enabled(Some("")), "empty must disable live mode");
    assert!(!live_mode_enabled(Some("0")), "MCPHOST_LIVE=0 must disable live mode");
    assert!(!live_mode_enabled(Some("true")), "only the literal '1' enables live mode");
    assert!(live_mode_enabled(Some("1")), "MCPHOST_LIVE=1 must enable live mode");
}
