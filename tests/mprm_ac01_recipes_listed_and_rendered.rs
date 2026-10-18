//! AC1 (P0) -- PRD-mcphost-prompts-from-published-tools. Keyless
//! `prompts/list` returns one prompt per shipped recipe, each with typed,
//! described arguments; `prompts/get` returns one `user` message of at least
//! 200 characters with every `{{slot}}` substituted.

use crate::common;
use common::{McpClient, TestServer};
use mcphost::prompts::{RECIPES, recipe_prompts};
use serde_json::{Value, json};

#[tokio::test]
async fn mprm_ac01_recipes_listed_and_rendered() {
    assert!(RECIPES.len() >= 3, "premise: three shipped recipes");
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    client.initialize().await;

    let listed = client.rpc("prompts/list", json!({})).await;
    let prompts = listed["result"]["prompts"].as_array().unwrap_or_else(|| panic!("no prompts array: {listed}"));
    assert!(prompts.len() >= 3, "{listed}");
    assert_eq!(prompts.len(), recipe_prompts().len(), "list equals recipe_prompts()");

    for p in prompts {
        let name = p["name"].as_str().expect("name");
        let args = p["arguments"].as_array().unwrap_or_else(|| panic!("{name}: no arguments"));
        assert!(!args.is_empty(), "{name}: needs >= 1 argument");
        let mut given = serde_json::Map::new();
        for a in args {
            let desc = a["description"].as_str().unwrap_or_default();
            assert!(!desc.is_empty(), "{name}.{}: empty description", a["name"]);
            given.insert(a["name"].as_str().expect("arg name").to_string(), json!(format!("val_{}", a["name"].as_str().unwrap())));
        }
        let got = client.rpc("prompts/get", json!({"name": name, "arguments": Value::Object(given.clone())})).await;
        let messages = got["result"]["messages"].as_array().unwrap_or_else(|| panic!("{name}: {got}"));
        assert_eq!(messages.len(), 1, "{name}: one message");
        assert_eq!(messages[0]["role"], "user");
        let text = messages[0]["content"]["text"].as_str().expect("text content");
        assert!(text.chars().count() >= 200, "{name}: only {} chars", text.chars().count());
        assert!(!text.contains("{{"), "{name}: unsubstituted slot in {text}");
        for v in given.values() {
            assert!(text.contains(v.as_str().unwrap()), "{name}: {v} not rendered into {text}");
        }
    }
}
