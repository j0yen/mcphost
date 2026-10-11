//! PRD-mcphost-connection-provider-registry AC4: `Provider::GOOGLE`'s
//! `validate_scopes(["https://www.googleapis.com/auth/spreadsheets.readonly",
//! "bogus"])` returns `Err(["bogus"])`; scope names are matched verbatim.

use mcphost::connect::Provider;

fn s(items: &[&str]) -> Vec<String> {
    items.iter().map(|x| x.to_string()).collect()
}

#[test]
fn google_rejects_only_the_unknown_scope() {
    let got = Provider::GOOGLE.validate_scopes(&s(&[
        "https://www.googleapis.com/auth/spreadsheets.readonly",
        "bogus",
    ]));
    assert_eq!(got, Err(vec!["bogus".to_string()]));
}

#[test]
fn known_scopes_pass_and_matching_is_verbatim() {
    assert_eq!(
        Provider::GOOGLE.validate_scopes(&s(&["https://www.googleapis.com/auth/spreadsheets.readonly"])),
        Ok(())
    );
    assert_eq!(Provider::GITHUB.validate_scopes(&s(&["repo", "read:user"])), Ok(()));
    // No normalisation: GitHub's `repo` is not Google's URL, and case counts.
    assert_eq!(
        Provider::GOOGLE.validate_scopes(&s(&["repo"])),
        Err(vec!["repo".to_string()])
    );
    assert_eq!(
        Provider::GITHUB.validate_scopes(&s(&["REPO", "repo", "REPO"])),
        Err(vec!["REPO".to_string(), "REPO".to_string()])
    );
}
