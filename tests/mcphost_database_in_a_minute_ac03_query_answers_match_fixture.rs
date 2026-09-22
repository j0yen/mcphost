//! PRD-mcphost-database-in-a-minute
//! AC3 — Given the published `query` tool, When asked the equality,
//! range, and count questions, Then each answer matches the fixture's
//! known value.

use crate::common;
use common::{TempDataDir, TestServer, python_kind_registry};
use mcphost::sandbox;
use tokio::process::Command;

async fn run_proof(mcphost_url: &str) -> (bool, String) {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/database-in-a-minute/proof.sh");
    let output = Command::new("bash")
        .arg(&script)
        .env("MCPHOST_URL", mcphost_url)
        .output()
        .await
        .expect("run examples/database-in-a-minute/proof.sh");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(!stdout.is_empty(), "proof.sh produced no stdout; stderr:\n{stderr}");
    (output.status.success(), stdout)
}

fn check_passed(stdout: &str, name: &str) -> bool {
    stdout.lines().any(|l| l == format!("CHECK {name}: PASS"))
}

fn value_of(stdout: &str, name: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{name}=")).map(str::to_string))
}

#[tokio::test]
async fn equality_range_and_count_questions_match_the_fixtures_known_values() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let mcphost_url = format!("{}/mcp", server.base_url);

    let (success, stdout) = run_proof(&mcphost_url).await;

    // Equality: order id=777's known amount, per fixture.csv's
    // deterministic generation formula (amount = round(9.99 + (id % 500) *
    // 1.11, 2)).
    assert!(
        check_passed(&stdout, "equality_question_matches_known_value"),
        "proof.sh did not confirm the equality question's known answer:\n{stdout}"
    );
    assert_eq!(
        value_of(&stdout, "EQUALITY_AMOUNT").as_deref(),
        Some("317.46"),
        "equality question (id=777) must answer the fixture's known amount:\n{stdout}"
    );

    // Range: rows with amount > 300, a known count over the whole fixture.
    assert!(
        check_passed(&stdout, "range_question_matches_known_value"),
        "proof.sh did not confirm the range question's known answer:\n{stdout}"
    );
    assert_eq!(
        value_of(&stdout, "RANGE_COUNT").as_deref(),
        Some("476"),
        "range question (amount > 300) must answer the fixture's known count:\n{stdout}"
    );

    // Count: rows in category "produce" -- the fixture cycles 5 categories
    // evenly over 1,000 rows, so each category is exactly 200.
    assert!(
        check_passed(&stdout, "count_question_matches_known_value"),
        "proof.sh did not confirm the count question's known answer:\n{stdout}"
    );
    assert_eq!(
        value_of(&stdout, "CATEGORY_COUNT").as_deref(),
        Some("200"),
        "count question (category=produce) must answer the fixture's known count:\n{stdout}"
    );

    assert!(success, "proof.sh exited non-zero overall:\n{stdout}");
}
