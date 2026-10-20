//! PRD-mcphost-connection-provider-registry AC1 (Falsifier): add a provider
//! `testprov` to the provider table and nothing else; every one of the five
//! surfaces must name it. `build.rs` really compiles `testprov` (the row in
//! `providers_test_support.toml`) into `Provider` under the `test-support`
//! feature, which this test binary is built with. So the assertions below
//! read the COMPILED enum, `Provider::ALL` and `all_specs()`, then run each
//! surface's own renderer over that compiled table. Without the build.rs
//! registration `Provider::Testprov` does not exist and this file does not
//! compile.

use mcphost::connect::{
    ConnectProviders, Provider, all_specs, callback_routes, codegen, committed_specs,
    doc_provider_names, effective_providers_toml, llms_provider_names, provider_enum, providers_json,
    render_connections_doc, render_llms_section, route_provider_names, PROVIDERS_TOML,
};

#[test]
fn a_new_table_row_is_compiled_into_the_enum_and_reaches_all_five_surfaces() {
    // Surface 2: the compiled `Provider` enum (a real variant, built by build.rs).
    let p = Provider::parse("testprov").expect("testprov compiled in under test-support");
    assert_eq!(p, Provider::Testprov);
    assert_eq!(p.name(), "testprov");
    assert!(Provider::ALL.contains(&Provider::Testprov));
    assert!(provider_enum().contains(&"testprov"), "schema enum is read from Provider::ALL");
    assert_eq!(provider_enum(), Provider::ALL.iter().map(|p| p.name()).collect::<Vec<_>>());
    assert!(Provider::Testprov.validate_scopes(&["testprov.read".to_string()]).is_ok());

    // The compiled table is what the generator emits for the effective TOML.
    let on_disk = std::fs::read_to_string(concat!(env!("OUT_DIR"), "/providers_gen.rs")).unwrap();
    assert_eq!(on_disk, codegen::render_rust(&codegen::parse_table(&effective_providers_toml()).unwrap()));

    let specs = all_specs();
    assert!(specs.iter().any(|s| s.name == "testprov"));

    // Surface 1: the `host.connect.providers` rows.
    let status = ConnectProviders::from_lookup(specs, |_| None);
    let rows = providers_json(specs, &status);
    let row = rows
        .iter()
        .find(|r| r["name"] == "testprov")
        .expect("host.connect.providers row for testprov");
    assert_eq!(row["scopes"], serde_json::json!(["testprov.read"]));
    assert_eq!(row["available"], false);

    // Surface 3: docs/connections.md.
    assert!(doc_provider_names(&render_connections_doc(specs)).contains("testprov"));

    // Surface 4: the llms.txt section.
    assert!(llms_provider_names(&render_llms_section(specs)).contains("testprov"));

    // Surface 5: the callback route table.
    assert!(route_provider_names(&callback_routes(specs)).contains("testprov"));
}

#[test]
fn the_committed_table_and_docs_have_no_testprov() {
    // Without test-support the table is the committed one; prod never lists it.
    assert!(!PROVIDERS_TOML.contains("testprov"));
    assert!(!codegen::effective_table(PROVIDERS_TOML, "[testprov]\n", false).contains("testprov"));
    assert!(committed_specs().iter().all(|s| s.name != "testprov"));
    assert!(!render_connections_doc(&committed_specs()).contains("testprov"));
}
