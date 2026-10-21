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
    assert!(text.contains("retry_s = 75\n"), "the data-sync get step is the retry_until_expect step");
    let parse = |t: &str| {
        let mut r: recipes::Recipe = toml::from_str(t).expect("parse recipe");
        r.name = "data-sync".to_string();
        r
    };

    // The key without retry_s is a recipe error naming the key.
    let bad = parse(&text.replace("retry_s = 75\n", ""));
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
}
