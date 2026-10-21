//! PRD-mcphost-incident-banner-for-agents
//! AC4 — Given `HOST_FAULT_CODES`, When the drift test runs, Then each
//! member is produced as `data.error_code` by some `AppError` value
//! constructed in the test, and `www/llms.txt` + the quickstart error guide
//! contain the literal `data.incident`.

use mcphost::errors::{AppError, HOST_FAULT_CODES, INCIDENT_FIELD};

fn error_code(err: AppError) -> String {
    err.into_error_data()
        .data
        .and_then(|d| d.get("error_code").and_then(|c| c.as_str().map(str::to_string)))
        .expect("data.error_code")
}

/// The refusal bundled SQLite gives a `BEGIN` inside a transaction.
fn nested_begin_error() -> rusqlite::Error {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("BEGIN").unwrap();
    conn.execute_batch("BEGIN").expect_err("nested BEGIN is refused")
}

#[test]
fn incban_ac04() {
    // One real AppError per host-fault code; the list below and
    // HOST_FAULT_CODES must name the same set.
    let produced: Vec<(&str, String)> = vec![
        ("storage", error_code(AppError::Storage("disk".into()))),
        ("internal", error_code(AppError::Internal("boom".into()))),
        ("exec_unavailable", error_code(AppError::exec_unavailable("pool down"))),
        ("timeout", error_code(AppError::host_timeout("deadline"))),
        ("rate_limited_host", error_code(AppError::rate_limited_host())),
        ("transaction_nested", error_code(AppError::from(nested_begin_error()))),
    ];
    for (listed, got) in &produced {
        assert_eq!(listed, got, "constructor for {listed} produced {got}");
    }
    for code in HOST_FAULT_CODES {
        assert!(produced.iter().any(|(_, got)| got == code), "{code} is produced by no AppError in this test");
    }
    for (listed, _) in &produced {
        assert!(HOST_FAULT_CODES.contains(listed), "{listed} is constructed here but missing from HOST_FAULT_CODES");
    }

    let literal = format!("data.{INCIDENT_FIELD}");
    assert_eq!(literal, "data.incident");
    let root = env!("CARGO_MANIFEST_DIR");
    for path in ["www/llms.txt", "docs/agent-quickstart.md"] {
        let text = std::fs::read_to_string(format!("{root}/{path}")).expect(path);
        assert!(text.contains(&literal), "{path} must name the literal `{literal}`");
        assert!(text.contains("retry_after_s"), "{path} must state the wait rule");
    }
}
