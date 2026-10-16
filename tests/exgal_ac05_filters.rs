//! AC5: `?use_case=nonsense` is 200 with zero cards and the 8 valid tags;
//! `?client=chatgpt` returns only examples whose `clients` contains `chatgpt`
//! or `any`.

use crate::common;
use common::TestServer;
use mcphost::examples::{EXAMPLE_USE_CASES, EXAMPLES};

async fn get(server: &TestServer, path: &str) -> (u16, String) {
    let r = reqwest::get(format!("{}{path}", server.base_url)).await.unwrap();
    (r.status().as_u16(), r.text().await.unwrap())
}

#[tokio::test]
async fn exgal_ac05_unknown_use_case_lists_the_valid_tags() {
    let server = TestServer::start().await;
    let (status, html) = get(&server, "/examples?use_case=nonsense").await;
    assert_eq!(status, 200);
    assert_eq!(html.matches("class=\"card\"").count(), 0);
    assert_eq!(EXAMPLE_USE_CASES.len(), 8);
    for tag in EXAMPLE_USE_CASES {
        assert!(html.contains(&format!("?use_case={tag}\">{tag}</a>")), "valid tag {tag} is listed");
    }
}

#[tokio::test]
async fn exgal_ac05_use_case_and_client_filters_select_matching_cards() {
    let server = TestServer::start().await;
    let (_, html) = get(&server, "/examples?use_case=docs-qa").await;
    let want: Vec<&str> = EXAMPLES.iter().filter(|e| e.use_case.contains(&"docs-qa")).map(|e| e.name).collect();
    assert!(want.len() >= 2);
    assert_eq!(html.matches("class=\"card\"").count(), want.len());

    let (status, html) = get(&server, "/examples?client=chatgpt").await;
    assert_eq!(status, 200);
    let want: Vec<&str> = EXAMPLES.iter().filter(|e| e.clients.iter().any(|c| *c == "chatgpt" || *c == "any")).map(|e| e.name).collect();
    assert_eq!(html.matches("class=\"card\"").count(), want.len());
    for ex in EXAMPLES {
        assert_eq!(html.contains(&format!("id=\"{}\"", ex.name)), want.contains(&ex.name), "{}", ex.name);
    }
}

#[test]
fn exgal_ac05_client_filter_excludes_examples_for_other_clients() {
    use mcphost::examples::{Example, ExampleFirstCall};
    use mcphost::examples_page::Filter;
    let ex = |clients: &'static [&'static str]| Example {
        name: "x",
        title: "x",
        summary: "x",
        use_case: &["chart"],
        clients,
        since: "0.1.0",
        proof: "proof.sh",
        secrets_needed: &[],
        tools: &[],
        first_call: ExampleFirstCall { tool: "t", args_json: "{}", expect: "result.a == 1" },
    };
    let f = Filter { use_case: None, client: Some("chatgpt".into()) };
    assert!(f.matches(&ex(&["chatgpt", "cursor"])));
    assert!(f.matches(&ex(&["any"])));
    assert!(!f.matches(&ex(&["cursor"])));
}
