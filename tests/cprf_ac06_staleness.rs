//! PRD-mcphost-client-profile-registry AC6 — Given a row whose `checked`
//! date is older than 90 days, When `cargo test` runs, Then the staleness
//! test fails naming the row and its `doc_url`.

use mcphost::client_profiles::{CLIENT_PROFILES, days_from_date, stale_rows, today};

#[test]
fn no_row_is_older_than_ninety_days() {
    let stale = stale_rows(&today());
    assert!(stale.is_empty(), "stale client profiles:\n{}", stale.join("\n"));
}

#[test]
fn a_row_past_ninety_days_is_named_with_its_doc_url() {
    let row = &CLIENT_PROFILES[1];
    let then = days_from_date(row.checked).expect("checked parses");
    // 91 days after the row's checked date, as a calendar date.
    let later = {
        let z = then + 91 + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        format!("{:04}-{m:02}-{d:02}", yoe + era * 400 + i64::from(m <= 2))
    };
    let stale = stale_rows(&later);
    assert_eq!(stale.len(), CLIENT_PROFILES.len(), "every row checked that day goes stale: {stale:?}");
    let mine = stale.iter().find(|m| m.contains(&format!("`{}`", row.id))).expect("row named");
    assert!(mine.contains(row.doc_url), "message names the doc_url: {mine}");
    assert!(stale_rows(row.checked).is_empty());
    assert_eq!(days_from_date("1970-01-02"), Some(1));
}
