//! PRD-mcphost-table-context-and-sql-passthrough
//! AC8 — Given the fixture loaded through `host.table.create` and
//! `host.table.append`, When `proof.sh` runs against a local mcphost,
//! Then the equality, range, count, GROUP BY sum by category, and LIKE
//! `'%prod%'` answers each match the fixture's known values and no
//! `host.tool_publish` call is made.
//!
//! Runs the real `examples/database-in-a-minute/proof.sh` against an
//! in-process `TestServer` over HTTP (the runnable proof the recipe
//! itself ships), then asserts on its `CHECK ...: PASS` lines the same
//! way `tests/mcphost_share_a_tool_not_a_key_ac02_*.rs` and the
//! predecessor PRD's own database-in-a-minute tests did. Unlike those
//! predecessor tests, this one needs no sandboxed-tool machinery at all:
//! the new recipe never publishes a tool, so plain `TestServer::start()`
//! (the built-in kind set only) is enough.

use tokio::process::Command;

use crate::common;
use common::TestServer;

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
async fn proof_sh_answers_five_sql_questions_and_never_publishes_a_tool() {
    let server = TestServer::start().await;
    let mcphost_url = format!("{}/mcp", server.base_url);

    let (success, stdout) = run_proof(&mcphost_url).await;

    assert!(
        check_passed(&stdout, "table_create"),
        "proof.sh did not confirm the table was created:\n{stdout}"
    );
    assert!(
        check_passed(&stdout, "batched_append_five_calls_of_200_succeed"),
        "proof.sh did not confirm the batched append succeeded:\n{stdout}"
    );
    assert!(
        check_passed(&stdout, "model_set_description"),
        "proof.sh did not confirm the model_set description call succeeded:\n{stdout}"
    );

    // Equality: id=777's known amount, per fixture.csv's deterministic
    // generation formula.
    assert!(
        check_passed(&stdout, "equality_question_matches_known_value"),
        "proof.sh did not confirm the equality question's known answer:\n{stdout}"
    );
    assert_eq!(value_of(&stdout, "EQUALITY_AMOUNT").as_deref(), Some("317.46"));

    // Range: rows with amount > 300, a known count over the whole fixture.
    assert!(
        check_passed(&stdout, "range_question_matches_known_value"),
        "proof.sh did not confirm the range question's known answer:\n{stdout}"
    );
    assert_eq!(value_of(&stdout, "RANGE_COUNT").as_deref(), Some("476"));

    // Count: rows in category "produce" -- the fixture cycles 5
    // categories evenly over 1,000 rows, so each category is exactly 200.
    assert!(
        check_passed(&stdout, "count_question_matches_known_value"),
        "proof.sh did not confirm the count question's known answer:\n{stdout}"
    );
    assert_eq!(value_of(&stdout, "CATEGORY_COUNT").as_deref(), Some("200"));

    // GROUP BY: sum of amount by category -- one row per of the 5
    // categories, something the old six-operator grammar could not
    // express at all.
    assert!(
        check_passed(&stdout, "group_by_question_returns_one_row_per_category"),
        "proof.sh did not confirm the GROUP BY question's known answer:\n{stdout}"
    );
    assert_eq!(value_of(&stdout, "GROUP_BY_ROW_COUNT").as_deref(), Some("5"));

    // LIKE: category names containing "prod" -- only "produce" matches,
    // so the count equals that category's own row count.
    assert!(
        check_passed(&stdout, "like_question_matches_known_value"),
        "proof.sh did not confirm the LIKE question's known answer:\n{stdout}"
    );
    assert_eq!(value_of(&stdout, "LIKE_COUNT").as_deref(), Some("200"));

    assert!(
        check_passed(&stdout, "query_log_holds_at_least_five_questions"),
        "proof.sh did not confirm the query log recorded its questions:\n{stdout}"
    );

    assert!(success, "proof.sh exited non-zero overall:\n{stdout}");

    // AC8: no host.tool_publish call is made -- the script's own source
    // never mentions the tool.
    let script_source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/database-in-a-minute/proof.sh"),
    )
    .expect("read proof.sh");
    assert!(
        !script_source.contains("host.tool_publish"),
        "proof.sh must not call host.tool_publish under the new SQL-passthrough recipe"
    );
}
