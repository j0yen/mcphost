//! PRD-mcphost-connection-provider-registry AC1 (Falsifier): add a provider
//! `testprov` to `providers.toml` and nothing else; every one of the five
//! surfaces must name it. The test appends one `[testprov]` block to the
//! committed `providers.toml` text, then drives the generator and each
//! surface's own renderer over the result -- the exact functions `build.rs`,
//! `gen-docs` and the `host.connect.providers` tool run -- so a surface that
//! needed a second hand edit to learn the name would be missing it here.

use mcphost::connect::{
    ConnectProviders, Provider, callback_routes, codegen, doc_provider_names, llms_provider_names,
    provider_enum, providers_json, render_connections_doc, render_llms_section, route_provider_names,
    specs_from_toml, PROVIDERS_TOML,
};

const TESTPROV_BLOCK: &str = r#"
[testprov]
auth_url = "http://127.0.0.1:0/authorize"
token_url = "http://127.0.0.1:0/token"
refresh = "rotating"
client_id_env = "MCPHOST_CONNECT_TESTPROV_CLIENT_ID"
client_secret_env = "MCPHOST_CONNECT_TESTPROV_CLIENT_SECRET"
docs_url = "http://127.0.0.1:0/docs"
scopes = [{ name = "testprov.read", purpose = "Read the in-repo test OAuth server" }]
"#;

fn extended_toml() -> String {
    format!("{PROVIDERS_TOML}\n{TESTPROV_BLOCK}")
}

#[test]
fn a_new_providers_toml_row_reaches_all_five_surfaces_with_no_other_edit() {
    let text = extended_toml();
    let specs = specs_from_toml(&text).expect("extended providers.toml parses");
    assert!(specs.iter().any(|s| s.name == "testprov"));

    // Surface 1: the `host.connect.providers` rows.
    let status = ConnectProviders::from_lookup(&specs, |_| None);
    let rows = providers_json(&specs, &status);
    let row = rows
        .iter()
        .find(|r| r["name"] == "testprov")
        .expect("host.connect.providers row for testprov");
    assert_eq!(row["scopes"], serde_json::json!(["testprov.read"]));
    assert_eq!(row["available"], false);

    // Surface 2: the `provider` enum is `Provider::ALL`, and the generator's
    // output for this table puts testprov in it (variant, ALL, parse).
    let rust = codegen::render_rust(&codegen::parse_table(&text).unwrap());
    assert!(rust.contains("    Testprov,\n"), "enum variant:\n{rust}");
    assert!(rust.contains("        Provider::Testprov,\n"), "Provider::ALL entry");
    assert!(rust.contains("\"testprov\" => Some(Provider::Testprov)"), "Provider::parse arm");
    assert_eq!(
        provider_enum(),
        Provider::ALL.iter().map(|p| p.name()).collect::<Vec<_>>(),
        "the schema enum is read from Provider::ALL, not a literal list"
    );

    // Surface 3: docs/connections.md.
    assert!(doc_provider_names(&render_connections_doc(&specs)).contains("testprov"));

    // Surface 4: the llms.txt section.
    assert!(llms_provider_names(&render_llms_section(&specs)).contains("testprov"));

    // Surface 5: the callback route table.
    assert!(route_provider_names(&callback_routes(&specs)).contains("testprov"));
}

#[test]
fn the_committed_table_has_no_testprov() {
    // The extra row exists only in this test's text; prod's list never shows it.
    assert!(Provider::parse("testprov").is_none());
    assert!(!PROVIDERS_TOML.contains("testprov"));
}
