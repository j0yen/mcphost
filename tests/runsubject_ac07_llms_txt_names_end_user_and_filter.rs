//! PRD-mcphost-runs-end-user-subject
//! AC7 (P0) — Given `www/llms.txt`, When the runs section is read, Then it
//! names `end_user` and the `end_user_subject` filter.

const LLMS_TXT: &str = include_str!("../www/llms.txt");

#[test]
fn llms_txt_names_end_user_and_its_filter() {
    assert!(LLMS_TXT.contains("end_user"), "llms.txt must name end_user");
    assert!(
        LLMS_TXT.contains("end_user_subject"),
        "llms.txt must name the end_user_subject filter"
    );
}
