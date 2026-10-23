//! contract: reads recipes/*.toml, tests/golden/recipes/**
//! `recipes_render_golden`: every recipe renders its `proof.sh`,
//! `synthorg-task.yaml` and `www/llms.txt` block byte-identically to the copy
//! checked in under `tests/golden/recipes/<recipe>/`, so a change to the
//! generator that moves any recipe's output shows up as a golden diff in
//! review. Regenerate one recipe's goldens with
//! `RECIPES_GOLDEN_UPDATE=<recipe> cargo test --test <suite> recipes_render_golden`
//! (`all` rewrites every recipe).

use mcphost::recipes;
use std::path::{Path, PathBuf};

fn repo() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn golden_dir(name: &str) -> PathBuf {
    repo().join("tests/golden/recipes").join(name)
}

#[test]
fn recipes_render_golden() {
    let update = std::env::var("RECIPES_GOLDEN_UPDATE").unwrap_or_default();
    let all = recipes::load_all(repo()).expect("load recipes");
    assert!(!all.is_empty(), "no recipes under recipes/");
    let mut drifted = Vec::new();
    for r in &all {
        let rendered = [
            ("proof.sh", recipes::render_proof(r)),
            ("synthorg-task.yaml", recipes::render_task_yaml(r)),
            ("llms-block.md", recipes::render_llms_block(r)),
        ];
        let dir = golden_dir(&r.name);
        for (file, got) in rendered {
            let path = dir.join(file);
            if update == "all" || update == r.name {
                std::fs::create_dir_all(&dir).expect("mkdir golden dir");
                std::fs::write(&path, &got).expect("write golden");
                continue;
            }
            let want = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e} (RECIPES_GOLDEN_UPDATE={} writes it)", path.display(), r.name));
            if let Some((line, golden_l, got_l)) = recipes::first_diff(&want, &got) {
                drifted.push(format!(
                    "recipe {}: tests/golden/recipes/{}/{file} line {line}: golden has {golden_l:?}, generator renders {got_l:?}",
                    r.name, r.name
                ));
            }
        }
    }
    assert!(drifted.is_empty(), "generator output moved off the goldens:\n{}", drifted.join("\n"));
}

#[test]
fn recipes_retry_until_expect_needs_retry_s_and_defines_the_poll_interval() {
    let text = std::fs::read_to_string(repo().join("recipes/data-sync.toml")).expect("read data-sync.toml");
    let parse = |t: &str| {
        let mut r: recipes::Recipe = toml::from_str(t).expect("parse recipe");
        r.name = "data-sync".to_string();
        r
    };
    let retry_line = format!("retry_s = {}\n", deadline_of(&parse(&text)));
    assert!(text.contains(&retry_line), "the data-sync get step is the retry_until_expect step");

    // The key without retry_s is a recipe error naming the key.
    let bad = parse(&text.replace(&retry_line, ""));
    let err = recipes::validate(&[bad]).expect_err("retry_until_expect without retry_s must be rejected").to_string();
    assert!(err.contains("retry_until_expect") && err.contains("requires `retry_s`"), "{err}");

    // The generator defines the poll interval the body sleeps on.
    let ok = parse(&text);
    recipes::validate(std::slice::from_ref(&ok)).expect("data-sync validates");
    let proof = recipes::render_proof(&ok);
    assert!(
        proof.contains(&format!("RETRY_UNTIL_EXPECT_POLL_S={}\n", recipes::RETRY_UNTIL_EXPECT_POLL_S)),
        "generated proof must define RETRY_UNTIL_EXPECT_POLL_S"
    );
    assert!(proof.contains("sleep \"$RETRY_UNTIL_EXPECT_POLL_S\""), "the body polls on the generated constant");
    assert_eq!(recipes::RETRY_UNTIL_EXPECT_POLL_S, 5);

    // The deadline is generated from retry_s, not typed into the body.
    let deadline = deadline_of(&ok);
    assert!(
        proof.contains(&format!("RETRY_UNTIL_EXPECT_DEADLINE_S={deadline}\n")),
        "generated proof must define RETRY_UNTIL_EXPECT_DEADLINE_S from retry_s"
    );
    assert!(proof.contains("+ RETRY_UNTIL_EXPECT_DEADLINE_S ))"), "the loop reads the generated deadline");
    let loop_body = proof.split("RETRY_UNTIL_EXPECT_DEADLINE_S=").nth(1).expect("const defined");
    let after_def = loop_body.split_once('\n').expect("line end").1;
    assert!(
        !after_def.split(|c: char| !c.is_ascii_digit()).any(|t| t == deadline.to_string()),
        "no bare {deadline} may follow the generated constant"
    );

    // A body that hard-codes the deadline is rejected.
    let hard = parse(&text.replace("+ RETRY_UNTIL_EXPECT_DEADLINE_S ))", &format!("+ {deadline} ))")));
    let err = recipes::validate(&[hard]).expect_err("hard-coded deadline must be rejected").to_string();
    assert!(err.contains("hard-codes the deadline"), "{err}");
}

fn deadline_of(r: &recipes::Recipe) -> u64 {
    r.steps.iter().find(|s| s.retry_until_expect).and_then(|s| s.retry_s).expect("a retry_until_expect step with retry_s")
}

/// The in-process runner kills any recipe at `MAX_RECIPE_WALL`
/// (tests/support/rgpt.rs): read the real constant, and fail any recipe whose
/// worst case (every polled step's retry_s plus one poll, plus the other
/// steps) leaves less than the margin under it, or exceeds its own wall_s.
#[test]
fn recipes_worst_case_fits_the_rgpt_cap() {
    let rgpt = std::fs::read_to_string(repo().join("tests/support/rgpt.rs")).expect("read rgpt.rs");
    // The cap is derived in rgpt.rs (not a literal): it must spell the same
    // formula, from the same real constants, that the validator uses.
    let flat: String = rgpt.split("pub const MAX_RECIPE_WALL").nth(1).expect("MAX_RECIPE_WALL in tests/support/rgpt.rs").split(';').next().unwrap_or("").split_whitespace().collect();
    for term in [
        "mcphost::recipes::MAX_POLLED_STEPS",
        "mcphost::triggers::SCHEDULER_TICK.as_secs()",
        "mcphost::triggers::SYNC_FETCH_ALLOWANCE_S",
        "mcphost::recipes::RETRY_UNTIL_EXPECT_POLL_S",
        "mcphost::recipes::RECIPE_NON_POLL_STEP_S",
        "mcphost::recipes::RECIPE_WALL_MARGIN_S",
    ] {
        assert!(flat.contains(term), "MAX_RECIPE_WALL in tests/support/rgpt.rs must be derived from {term}: {flat}");
    }
    let secs = recipes::MAX_POLLED_STEPS as u64
        * (mcphost::triggers::SCHEDULER_TICK.as_secs() + mcphost::triggers::SYNC_FETCH_ALLOWANCE_S + recipes::RETRY_UNTIL_EXPECT_POLL_S)
        + recipes::RECIPE_NON_POLL_STEP_S
        + recipes::RECIPE_WALL_MARGIN_S;
    assert_eq!(secs, recipes::RGPT_MAX_RECIPE_WALL_S, "RGPT_MAX_RECIPE_WALL_S drifted from tests/support/rgpt.rs");

    let all = recipes::load_all(repo()).expect("load recipes");
    let polled: Vec<_> = all.iter().filter(|r| r.steps.iter().any(|s| s.retry_until_expect)).collect();
    assert!(!polled.is_empty(), "no recipe uses retry_until_expect; the worst-case guard has nothing to check");
    for r in polled {
        let worst = recipes::retry_until_expect_worst_case_s(r);
        let retry_s = deadline_of(r);
        let polls = r.steps.iter().filter(|s| s.retry_until_expect).count() as u64;
        assert_eq!(worst, polls * (retry_s + recipes::RETRY_UNTIL_EXPECT_POLL_S) + recipes::RECIPE_NON_POLL_STEP_S, "{}", r.name);
        assert!(
            worst + recipes::RECIPE_WALL_MARGIN_S <= secs,
            "recipe {}: worst case {worst} s (retry_s {retry_s} + poll {} + other steps {}) is within {} s of the {secs} s rgpt cap",
            r.name,
            recipes::RETRY_UNTIL_EXPECT_POLL_S,
            recipes::RECIPE_NON_POLL_STEP_S,
            recipes::RECIPE_WALL_MARGIN_S,
        );
        assert!(worst <= u64::from(r.budget.wall_s), "recipe {}: worst case {worst} s exceeds its wall_s {}", r.name, r.budget.wall_s);
    }

    // The guard fails a recipe whose retry_s is too long for the cap.
    let text = std::fs::read_to_string(repo().join("recipes/data-sync.toml")).expect("read data-sync.toml");
    let mut r: recipes::Recipe = toml::from_str(&text).expect("parse recipe");
    r.name = "data-sync".to_string();
    let too_long = secs; // retry_s alone already fills the cap
    for st in r.steps.iter_mut().filter(|s| s.retry_until_expect) {
        st.retry_s = Some(too_long);
    }
    let err = recipes::validate(&[r]).expect_err("a worst case over the cap must be rejected").to_string();
    assert!(err.contains("in-process cap"), "{err}");
}

/// A present-but-empty `llms_call` is a recipe error naming the key; a
/// non-empty one is the call text the llms block renders.
#[test]
fn recipes_llms_call_must_not_be_empty_and_renders_into_the_block() {
    let text = std::fs::read_to_string(repo().join("recipes/data-sync.toml")).expect("read data-sync.toml");
    let mut r: recipes::Recipe = toml::from_str(&text).expect("parse recipe");
    r.name = "data-sync".to_string();
    let call = r.steps.iter().find_map(|s| s.llms_call.clone()).expect("data-sync sets llms_call on create");
    assert!(!call.trim().is_empty());
    recipes::validate(std::slice::from_ref(&r)).expect("data-sync validates");
    assert!(recipes::render_llms_block(&r).contains(&call), "llms block must contain the llms_call text");

    for empty in ["", "  "] {
        let mut bad = r.clone();
        for s in bad.steps.iter_mut().filter(|s| s.llms_call.is_some()) {
            s.llms_call = Some(empty.to_string());
        }
        let err = recipes::validate(&[bad]).expect_err("empty llms_call must be rejected").to_string();
        assert!(err.contains("llms_call"), "error must name the key: {err}");
    }
}
