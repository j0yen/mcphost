//! PRD-mcphost-connection-provider-registry AC4 (P0) -- Given
//! `Provider::GOOGLE`, When `validate_scopes(["https://www.googleapis.com/auth/
//! spreadsheets.readonly", "bogus"])` runs, Then it returns `Err(["bogus"])`.

use mcphost::connect::Provider;

#[test]
fn cprov_ac04() {
    let scopes = vec!["https://www.googleapis.com/auth/spreadsheets.readonly".to_string(), "bogus".to_string()];
    assert_eq!(Provider::GOOGLE.validate_scopes(&scopes), Err(vec!["bogus".to_string()]));
    assert_eq!(Provider::GOOGLE.validate_scopes(&scopes[..1]), Ok(()));
    assert_eq!(Provider::GITHUB.validate_scopes(&["repo".to_string()]), Ok(()));
}
