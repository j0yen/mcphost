//! PRD-mcphost-example-proof-nightly-and-hide
//! AC5 — Given a report for a name not in the embedded index, When
//! `admin.example_proof_report` runs, Then it returns `example_not_found`
//! and writes nothing; given a non-admin bearer, Then `forbidden`.

use crate::exprf;

#[tokio::test]
async fn exprf_ac05() {
    let f = exprf::start().await;

    let err = exprf::report(&f.admin, "no-such-example", false).await.expect_err("unknown name refused");
    assert_eq!(err.error_code.as_deref(), Some("example_not_found"), "{} {}", err.code, err.message);
    assert!(f.server.state.db.list_example_proofs().await.expect("rows").is_empty(), "nothing written");
    assert!(f.server.state.db.list_open_incidents().await.expect("open").is_empty(), "no incident opened");

    let known = exprf::names()[0];
    let err = exprf::report(&f.tenant, known, false).await.expect_err("tenant bearer refused");
    assert_eq!(err.error_code.as_deref(), Some("forbidden"), "{} {}", err.code, err.message);
    assert!(f.server.state.db.list_example_proofs().await.expect("rows").is_empty(), "nothing written by a tenant");
}
