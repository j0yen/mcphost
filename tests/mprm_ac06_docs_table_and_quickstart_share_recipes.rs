//! contract: reads scripts/sandbox-api-doc-check.sh
//! AC6 (P0) -- PRD-mcphost-prompts-from-published-tools.
//! `docs/protocol/prompts.md` equals `render_prompts_doc()` (built from
//! `recipe_prompts()`) and `scripts/sandbox-api-doc-check.sh` passes only
//! then; `host.quickstart {kind}` returns each recipe's text from the same
//! `RECIPES` table the prompts read.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use mcphost::kinds::aliases::KIND_ALIASES;
use mcphost::kinds::outcomes::OUTCOMES;
use mcphost::prompts::{RECIPES, recipe_text, render_prompts_doc};
use serde_json::{Value, json};
use std::path::Path;
use std::process::{Command, Output};

const DOC: &str = "docs/protocol/prompts.md";

fn run_check(dir: &Path) -> Output {
    Command::new("bash")
        .arg(dir.join("scripts/sandbox-api-doc-check.sh"))
        .current_dir(dir)
        .env("MCPHOST_BIN", env!("CARGO_BIN_EXE_mcphost"))
        .output()
        .expect("run sandbox-api-doc-check.sh")
}

#[test]
fn mprm_ac06_committed_doc_equals_the_rendered_table() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(root.join(DOC)).expect("read docs/protocol/prompts.md");
    assert_eq!(text, render_prompts_doc(), "{DOC} drifted from recipe_prompts() rendered");
    for r in RECIPES {
        assert!(text.contains(&format!("| `{}` |", r.name)), "{DOC} lacks recipe {}", r.name);
    }
}

#[test]
fn mprm_ac06_check_script_passes_clean_and_fails_naming_the_doc_when_a_row_is_edited() {
    let clean = common::doc_check_scratch("mprm-ac06-clean");
    let out = run_check(&clean);
    assert!(out.status.success(), "clean tree must exit 0, stderr:\n{}", String::from_utf8_lossy(&out.stderr));
    std::fs::remove_dir_all(&clean).ok();

    let dir = common::doc_check_scratch("mprm-ac06-edited");
    let path = dir.join(DOC);
    let text = std::fs::read_to_string(&path).expect("read");
    let edited = text.replace("`webhook-inbox` |", "`webhook-inbox-renamed` |");
    assert_ne!(edited, text, "the edit must change the table");
    std::fs::write(&path, edited).expect("write");
    let out = run_check(&dir);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "an edited row must exit 1, stderr:\n{stderr}");
    assert!(stderr.contains(DOC), "stderr must name the doc:\n{stderr}");
    std::fs::remove_dir_all(&dir).ok();
}

/// A `kind` word `host.quickstart` routes to `recipe`: an alias or an outcome word.
fn word_for(recipe: &str) -> String {
    if let Some(a) = KIND_ALIASES.iter().find(|a| a.recipe == recipe) {
        return a.alias.to_string();
    }
    let o = OUTCOMES.iter().find(|o| o.recipe == recipe).unwrap_or_else(|| panic!("no kind word routes to recipe {recipe}"));
    o.words[0].to_string()
}

#[tokio::test]
async fn mprm_ac06_quickstart_returns_each_recipes_table_text() {
    let server = TestServer::start_with_kinds(common::http_kind_registry()).await;
    let (_ns, key) = common::signup(&server.base_url, "mprm ac06").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    for r in RECIPES {
        let word = word_for(r.name);
        let result = client.tools_call("host.quickstart", json!({"kind": word})).await.unwrap_or_else(|e| panic!("{word}: {e:?}"));
        let body: Value = extract_structured(&result);
        let text = body.pointer("/recipe/text").or_else(|| body.get("text")).and_then(Value::as_str).unwrap_or_else(|| panic!("{word}: no recipe text in {body}"));
        assert_eq!(Some(text.to_string()), recipe_text(r.name), "{word}: quickstart text is the RECIPES text");
        assert!(text.contains(r.arguments[0].0), "{word}: slots shown as <name>");
    }
}
