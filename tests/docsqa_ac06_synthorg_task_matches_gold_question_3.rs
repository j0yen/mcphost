//! PRD-mcphost-docs-qa-recipe
//! AC6 -- Given `~/repos/synthorg` with the new task, When
//! `synthorg consume --preflight` and the proxy-tier fixture run execute,
//! Then the `docs_qa_recipe` task is listed for `rag_indexer` and the fake
//! endpoint fixture passes its gold predicate.
//!
//! `~/repos/synthorg` is a separate repository outside this worktree's own
//! `cargo test` gate (no `uv`/python toolchain on the runner box), so the
//! actual `synthorg consume --preflight` run and the fake-mode
//! `pytest`/`--live-shape` proof that the new `/docs/search/question-3`
//! fixture route genuinely satisfies `docs_qa_recipe`'s own
//! `gold.call_check` were run by hand in that repo and recorded, with the
//! synthorg commit sha and real command output, in
//! `docs/receipts/docsqa-ac06-synthorg-preflight.md` -- same "Live, evidence
//! recorded outside this gate" shape as every other cross-repo AC in this
//! recipe family. `receipt_records_the_real_cross_repo_run` below fails if
//! that receipt goes missing or stops naming the actual run. The
//! per-repo half this worktree's own gate CAN prove: `examples/docs-qa/
//! synthorg-task.yaml` (kept byte-identical to what was actually merged
//! into `~/repos/synthorg/corpora/mcphost/consumer-tasks.yaml`) names the
//! `rag_indexer` segment, publishes `ask_docs`, and its own `call_check`
//! predicate names the exact document `examples/docs-qa/corpus/gold.json`'s
//! own question 3 expects -- so the two repos' copies of "question 3" can
//! never silently drift apart.

use crate::docs_qa;

use serde_json::Value;
use std::path::Path;

struct SynthorgTask {
    id: String,
    segments: String,
    tool_name: String,
    call_check: String,
    kind: String,
    fixture_route: String,
}

/// Hand-rolled rather than pulling in a YAML crate for one small,
/// self-owned fixture -- same precedent as
/// `mcphost_uptime_probes_ac07_synthorg_task_five_completions.rs`'s own
/// `load_gold`.
fn load_synthorg_task() -> SynthorgTask {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/docs-qa/synthorg-task.yaml");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

    let mut id = None;
    let mut segments = None;
    let mut tool_name = None;
    let mut call_check = None;
    let mut kind = None;
    let mut fixture_route = None;
    let mut in_gold = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(v) = trimmed.strip_prefix("- id:") {
            id = Some(v.trim().to_string());
            continue;
        }
        if let Some(v) = trimmed.strip_prefix("segments:") {
            segments = Some(v.trim().to_string());
            continue;
        }
        if trimmed == "gold:" {
            in_gold = true;
            continue;
        }
        if let Some(v) = trimmed.strip_prefix("fixture_route:") {
            fixture_route = Some(v.trim().to_string());
            continue;
        }
        if !in_gold {
            continue;
        }
        if let Some(v) = trimmed.strip_prefix("tool_name:") {
            tool_name = Some(v.trim().to_string());
        } else if let Some(v) = trimmed.strip_prefix("call_check:") {
            call_check = Some(v.trim().trim_matches('"').to_string());
        } else if let Some(v) = trimmed.strip_prefix("kind:") {
            kind = Some(v.trim().to_string());
        }
    }
    SynthorgTask {
        id: id.expect("synthorg-task.yaml: id"),
        segments: segments.expect("synthorg-task.yaml: segments"),
        tool_name: tool_name.expect("synthorg-task.yaml: gold.tool_name"),
        call_check: call_check.expect("synthorg-task.yaml: gold.call_check"),
        kind: kind.expect("synthorg-task.yaml: gold.kind"),
        fixture_route: fixture_route.expect("synthorg-task.yaml: fixture_route"),
    }
}

fn gold_question_3_expected_document() -> String {
    let path = docs_qa::corpus_dir().join("gold.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let gold: Value = serde_json::from_str(&text).expect("parse gold.json");
    let q3 = gold
        .as_array()
        .expect("gold.json array")
        .iter()
        .find(|q| q["id"] == 3)
        .expect("gold.json must have a question with id 3");
    q3["expected_document"].as_str().expect("expected_document").to_string()
}

#[test]
fn synthorg_task_is_listed_for_rag_indexer_and_names_question_3s_document() {
    let task = load_synthorg_task();
    assert_eq!(task.id, "docs_qa_recipe", "requirement 5: the task's own id");
    assert!(
        task.segments.contains("rag_indexer"),
        "requirement 5: the task must be listed for rag_indexer, got segments: {}",
        task.segments
    );
    assert_eq!(task.tool_name, "ask_docs", "requirement 5: gold.tool_name must be ask_docs");
    assert_eq!(task.kind, "http");
    assert!(
        task.fixture_route.starts_with('/'),
        "fixture_route must be an absolute path: {}",
        task.fixture_route
    );

    let expected_document = gold_question_3_expected_document();
    assert!(
        task.call_check.contains(&expected_document),
        "the synthorg task's own call_check ({:?}) must name the exact document \
         examples/docs-qa/corpus/gold.json's question 3 expects ({expected_document:?}) -- \
         \"top5 hit on question 3\" (requirement 5) means THIS question, not an arbitrary one",
        task.call_check
    );
}

/// AC6's own When-clause (`synthorg consume --preflight` and the proxy-tier
/// fixture run) executes in a separate repo this gate cannot invoke
/// (`~/repos/synthorg`, no python toolchain on the runner box). This test
/// owns the half that IS provable here: that the recorded evidence exists,
/// names the real synthorg commit that carries `docs_qa_recipe`, and reports
/// the same document this repo's own gold.json expects for question 3 --
/// so the receipt cannot silently drift from either repo's own facts.
#[test]
fn receipt_records_the_real_cross_repo_run() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/receipts/docsqa-ac06-synthorg-preflight.md");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

    for needle in [
        "docs_qa_recipe",
        "rag_indexer",
        "eb33096",
        "synthorg consume --preflight",
        "corpus check --live-shape",
        "ok — every gold tool is served",
    ] {
        assert!(text.contains(needle), "{} must record {needle:?}: {text}", path.display());
    }

    let expected_document = gold_question_3_expected_document();
    assert!(
        text.contains(&expected_document),
        "{} must name the same document gold.json's question 3 expects ({expected_document:?}): {text}",
        path.display()
    );
}
