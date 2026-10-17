//! PRD-mcphost-recipe-golden-path-tests
//! AC6 -- Given the synthorg corpus with one recipe task whose
//! `wall_budget_s` differs from the generated yaml, When
//! `scripts/recipe-task-sync-check.sh` runs, Then it exits 1 and prints the
//! task id and both values; with no drift it exits 0.

use crate::common;

use common::TempDataDir;
use std::path::Path;
use std::process::Command;

const TASK_ID: &str = "recipe_chart_in_a_minute";

/// The generated chart-in-a-minute yaml, retagged as a corpus task, with its
/// `wall_budget_s` optionally replaced.
fn corpus_with_wall_budget(dir: &TempDataDir, wall_budget_s: Option<u32>) -> std::path::PathBuf {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let generated = std::fs::read_to_string(repo.join("examples/chart-in-a-minute/synthorg-task.yaml")).expect("generated yaml");
    let mut corpus = String::new();
    let mut saw_budget = false;
    for line in generated.lines() {
        if line.starts_with("  - id: ") {
            corpus.push_str(&format!("  - id: {TASK_ID}\n"));
        } else if line.trim_start().starts_with("wall_budget_s:") {
            saw_budget = true;
            let value = wall_budget_s.map_or_else(|| line.trim_start().trim_start_matches("wall_budget_s:").trim().to_string(), |v| v.to_string());
            corpus.push_str(&format!("    wall_budget_s: {value}\n"));
        } else {
            corpus.push_str(line);
            corpus.push('\n');
        }
    }
    assert!(saw_budget, "the generated yaml must carry wall_budget_s");
    let path = dir.0.join("consumer-tasks.yaml");
    std::fs::write(&path, corpus).expect("write corpus");
    path
}

fn run(corpus: &Path) -> (i32, String) {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/recipe-task-sync-check.sh");
    let out = Command::new("bash").arg(script).arg(corpus).output().expect("run recipe-task-sync-check.sh");
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    (out.status.code().unwrap_or(-1), text)
}

#[test]
fn rgpt_ac06_no_drift_exits_zero() {
    let dir = TempDataDir::new();
    let (code, text) = run(&corpus_with_wall_budget(&dir, None));
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("1 task(s) checked, 0 drifted"), "{text}");
}

#[test]
fn rgpt_ac06_wall_budget_drift_exits_one_naming_the_task_and_both_values() {
    let recipes = mcphost::recipes::load_all(Path::new(env!("CARGO_MANIFEST_DIR"))).expect("recipes");
    let generated = recipes.iter().find(|r| r.name == "chart-in-a-minute").expect("chart recipe").budget.wall_s;
    let drifted = generated + 30;

    let dir = TempDataDir::new();
    let (code, text) = run(&corpus_with_wall_budget(&dir, Some(drifted)));
    assert_eq!(code, 1, "{text}");
    assert!(text.contains(TASK_ID), "{text}");
    assert!(text.contains("wall_budget_s"), "{text}");
    assert!(text.contains(&format!("corpus={drifted}")), "{text}");
    assert!(text.contains(&format!("generated={generated}")), "{text}");
}
