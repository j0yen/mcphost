//! AC1 (P0) -- Given the 10 fixture sentences and the `FakeDrafter`
//! replaying each recorded first draft, When each draft runs through
//! `spec_test`, Then >= 8 pass on the first attempt and the test prints the
//! per-sentence pass table (the falsifier as a test).

use crate::common;
use common::{McpClient, TestServer, extract_structured, poll_spec_test_until_ready, python_kind_registry, signup};
use mcphost::drafter::{Drafter, FakeDrafter, spec_test_verdict};
use mcphost::sandbox;
use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;

#[tokio::test]
async fn at_least_eight_of_ten_recorded_first_drafts_pass_spec_test() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/draft");
    let sentences: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(dir.join("sentences.json")).expect("sentences.json"))
            .expect("sentences parse");
    assert_eq!(sentences.len(), 10, "the PRD fixes the corpus at 10 sentences");

    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "TFD AC1 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let mut table = String::from("sentence | pass\n");
    let mut passed = 0;
    for s in &sentences {
        let id = s["id"].as_str().expect("id");
        let fake = FakeDrafter::from_fixture(&dir, id).expect("fixture");
        let draft = fake.draft("replayed").await.expect("first draft");
        let result = poll_spec_test_until_ready(
            &client,
            json!({"kind": "python", "spec": draft.spec, "invocations": draft.invocations}),
            Duration::from_secs(10),
        )
        .await;
        let ok = spec_test_verdict(&extract_structured(&result)).is_ok();
        passed += usize::from(ok);
        table.push_str(&format!("{id} | {}\n", if ok { "PASS" } else { "FAIL" }));
    }
    println!("{table}{passed}/10 passed first attempt");
    assert!(passed >= 8, "falsifier: {passed}/10 first drafts passed\n{table}");
}
