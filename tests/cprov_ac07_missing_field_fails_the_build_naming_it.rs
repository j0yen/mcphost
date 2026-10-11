//! PRD-mcphost-connection-provider-registry AC7 (P0): a `providers.toml` row
//! missing `token_url` makes `build.rs` fail naming the provider and the
//! field. `build.rs` parses with `connect::codegen::parse_table` (it
//! `#[path]`-includes this very file) and panics with the error string, so
//! the failing build's message IS `parse_table`'s `Err`; these tests pin that
//! message for every required field and pin the wiring in `build.rs`.

use mcphost::connect::{PROVIDERS_TOML, codegen};

fn block_without(field: &str) -> String {
    let all = [
        ("auth_url", "auth_url = \"https://x.example/a\""),
        ("token_url", "token_url = \"https://x.example/t\""),
        ("refresh", "refresh = \"none\""),
        ("client_id_env", "client_id_env = \"X_ID\""),
        ("client_secret_env", "client_secret_env = \"X_SECRET\""),
        ("docs_url", "docs_url = \"https://x.example/d\""),
        ("scopes", "scopes = []"),
    ];
    let body: Vec<&str> = all.iter().filter(|(k, _)| *k != field).map(|(_, l)| *l).collect();
    format!("{PROVIDERS_TOML}\n[brokenprov]\n{}\n", body.join("\n"))
}

#[test]
fn a_row_missing_token_url_names_the_provider_and_the_field() {
    let err = codegen::parse_table(&block_without("token_url")).expect_err("must fail");
    assert!(err.contains("brokenprov"), "names the provider: {err}");
    assert!(err.contains("token_url"), "names the field: {err}");
}

#[test]
fn every_required_field_is_enforced_by_name() {
    for field in ["auth_url", "token_url", "refresh", "client_id_env", "client_secret_env", "docs_url", "scopes"] {
        let err = codegen::parse_table(&block_without(field)).expect_err(field);
        assert!(err.contains("brokenprov") && err.contains(field), "{field}: {err}");
    }
}

#[test]
fn a_bad_refresh_kind_is_rejected_and_the_committed_table_parses() {
    let bad = PROVIDERS_TOML.replacen("refresh = \"none\"", "refresh = \"sometimes\"", 1);
    let err = codegen::parse_table(&bad).expect_err("bad refresh");
    assert!(err.contains("refresh") && err.contains("sometimes"), "{err}");
    assert!(codegen::parse_table(PROVIDERS_TOML).is_ok());
}

#[test]
fn build_rs_fails_the_build_with_the_parse_error() {
    let build_rs = include_str!("../build.rs");
    assert!(build_rs.contains("codegen::parse_table(&text)"), "build.rs must call the shared parser");
    assert!(build_rs.contains("Err(e) => panic!(\"{e}\")"), "build.rs must fail the build with its message");
}

/// R2: the compiled table is exactly what the generator emits for the
/// committed `providers.toml`, so a hand-written `providers_gen.rs` cannot
/// stand in for it.
#[test]
fn compiled_table_equals_a_fresh_generator_run() {
    let fresh = codegen::render_rust(&codegen::parse_table(PROVIDERS_TOML).unwrap());
    let on_disk = std::fs::read_to_string(concat!(env!("OUT_DIR"), "/providers_gen.rs")).expect("OUT_DIR file");
    assert_eq!(on_disk, fresh);
}
