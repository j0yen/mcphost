//! PRD-mcphost-recipe-golden-path-tests
//! AC4 -- Given the webhook-inbox toml, When generated, Then `www/llms.txt`
//! contains a webhook section, `examples/webhook-inbox/proof.sh` exists, and
//! `tests/rgpt_webhook_inbox_golden_path.rs` passes against `TestServer` with
//! the signed-POST fixture.
//!
//! The golden file is a generated `#[tokio::test]` in this same suite; this
//! file checks the three generated artifacts exist and then drives the
//! recipe's spec itself, so the signed-POST path is proven here by name.

use crate::rgpt;

use mcphost::recipes;
use std::path::Path;

fn repo() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn webhook() -> recipes::Recipe {
    recipes::load_all(repo())
        .expect("load recipes")
        .into_iter()
        .find(|r| r.name == "webhook-inbox")
        .expect("recipes/webhook-inbox.toml")
}

#[test]
fn rgpt_ac04_generated_artifacts_exist() {
    let llms = std::fs::read_to_string(repo().join("www/llms.txt")).expect("llms.txt");
    assert!(llms.contains(&recipes::begin_marker("webhook-inbox")), "no webhook block in www/llms.txt");
    let section = &llms[llms.find("## Recipe calls: Webhook inbox in one call").expect("webhook section")..];
    assert!(section.contains("host.webhook_inbox.create(name=\"inbox\")"), "{section}");
    assert!(section.contains("signed"), "the block must show the signed POST: {section}");

    let proof = repo().join("examples/webhook-inbox/proof.sh");
    let text = std::fs::read_to_string(&proof).expect("examples/webhook-inbox/proof.sh");
    assert!(text.contains("GENERATED from recipes/webhook-inbox.toml"), "proof.sh is not generated");

    let golden = std::fs::read_to_string(repo().join("tests/rgpt_webhook_inbox_golden_path.rs"))
        .expect("tests/rgpt_webhook_inbox_golden_path.rs");
    assert!(golden.contains("GENERATED from recipes/webhook-inbox.toml"), "golden test is not generated");
    assert!(golden.contains("signed_post"), "golden test must carry the signed-POST fixture");
}

#[tokio::test]
async fn rgpt_ac04_golden_path_passes_with_the_signed_post_fixture() {
    if mcphost::sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", mcphost::sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let recipe = webhook();
    let posts: Vec<_> = recipe.steps.iter().filter_map(|s| s.signed_post.as_ref()).collect();
    assert_eq!(posts.len(), 2, "one signed and one unsigned POST");
    assert!(posts[0].signed && posts[0].status == 200, "{posts:?}");
    assert!(!posts[1].signed && posts[1].status == 401, "{posts:?}");

    let trace = rgpt::run(&recipes::golden_spec(&recipe)).await;
    assert_eq!(
        trace.calls,
        ["host.webhook_inbox.create", "signed_post", "signed_post", "host.tool_call", "host.msg.wait"],
        "the recipe's calls must run in order"
    );
}
