//! PRD-mcphost-recipe-golden-path-tests
//! AC3 -- Given a step naming a tool absent from the keyed `tools/list` or
//! an argument key absent from the tool's input schema, When the generator
//! runs, Then it exits 2 and names recipe, step index and field.

use crate::common;

use common::TempDataDir;
use mcphost::recipes::{self, RecipeError};
use std::path::Path;

const RECIPE: &str = "chart-in-a-minute";

/// A scratch root whose `recipes/chart-in-a-minute.toml` is the committed one
/// with `from` replaced by `to`.
fn scratch_with_edit(from: &str, to: &str) -> TempDataDir {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch = TempDataDir::new();
    std::fs::create_dir_all(scratch.0.join("recipes")).expect("mkdir recipes");
    std::fs::create_dir_all(scratch.0.join("www")).expect("mkdir www");
    std::fs::copy(repo.join("www/llms.txt"), scratch.0.join("www/llms.txt")).expect("copy llms.txt");
    let src = std::fs::read_to_string(repo.join(format!("recipes/{RECIPE}.toml"))).expect("read toml");
    assert!(src.contains(from), "fixture edit target {from:?} is gone from the toml");
    std::fs::write(scratch.0.join(format!("recipes/{RECIPE}.toml")), src.replacen(from, to, 1)).expect("write toml");
    scratch
}

fn invalid(scratch: &TempDataDir) -> (String, usize, String) {
    match recipes::plan(&scratch.0) {
        Err(RecipeError::Invalid { recipe, step, field, .. }) => (recipe, step, field),
        other => panic!("expected an Invalid error, got {other:?}"),
    }
}

#[test]
fn rgpt_ac03_unknown_tool_exits_2_naming_recipe_step_and_field() {
    let scratch = scratch_with_edit("tool = \"host.table.append\"", "tool = \"host.table.nope\"");
    let (recipe, step, field) = invalid(&scratch);
    assert_eq!((recipe.as_str(), step, field.as_str()), (RECIPE, 2, "tool"));
    assert_eq!(recipes::cli(&scratch.0, &[]), 2);
    let msg = recipes::plan(&scratch.0).expect_err("plan must fail").to_string();
    assert!(msg.contains(RECIPE) && msg.contains("step 2") && msg.contains("field tool"), "{msg}");
    assert!(msg.contains("host.table.nope"), "{msg}");
}

#[test]
fn rgpt_ac03_unknown_argument_key_exits_2_naming_recipe_step_and_field() {
    let scratch = scratch_with_edit("args = { name = \"expenses\"", "args = { bogus = 1, name = \"expenses\"");
    let (recipe, step, field) = invalid(&scratch);
    assert_eq!((recipe.as_str(), step, field.as_str()), (RECIPE, 1, "args.bogus"));
    assert_eq!(recipes::cli(&scratch.0, &[]), 2);
    let msg = recipes::plan(&scratch.0).expect_err("plan must fail").to_string();
    assert!(msg.contains(RECIPE) && msg.contains("step 1") && msg.contains("field args.bogus"), "{msg}");
}

#[test]
fn rgpt_ac03_committed_recipes_validate() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let all = recipes::load_all(repo).expect("load");
    recipes::validate(&all).expect("every committed step names a real tool and real argument keys");
}
