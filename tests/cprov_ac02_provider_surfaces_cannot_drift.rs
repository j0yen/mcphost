//! PRD-mcphost-connection-provider-registry AC2 (P0) -- Given a hand edit that
//! adds "fakeprov" to any one of the five surfaces, When this test runs, Then
//! it fails naming the surface and the extra name.
//!
//! The clean tree must pass; then each surface gets a planted "fakeprov" and
//! the same check must refuse it, naming that surface. The committed files are
//! compared against the production rows (a production build never lists the
//! `test_only` row); the live surfaces against the full generated table.

use mcphost::connect::{self, Provider};

fn read(rel: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel))
        .unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn live_names() -> Vec<String> {
    Provider::ALL.iter().map(|p| p.name.to_string()).collect()
}

fn public_names() -> Vec<String> {
    Provider::public().map(|p| p.name.to_string()).collect()
}

/// Provider names in the `provider` enum of the real `host.connect.providers`
/// descriptor.
fn enum_names() -> Vec<String> {
    let kinds = mcphost::kinds::KindRegistry::with_builtin();
    let tools = mcphost::handler::host_tool_descriptors(&kinds);
    let tool = tools.iter().find(|t| t.name == "host.connect.providers").expect("host.connect.providers");
    tool.input_schema["properties"]["provider"]["enum"]
        .as_array()
        .expect("provider enum")
        .iter()
        .map(|v| v.as_str().expect("name").to_string())
        .collect()
}

fn tool_names() -> Vec<String> {
    connect::providers_json(None, &|_| None)["providers"]
        .as_array()
        .expect("providers")
        .iter()
        .map(|p| p["name"].as_str().expect("name").to_string())
        .collect()
}

/// (surface, names, expected set) for the five surfaces, optionally with
/// "fakeprov" hand-added to `plant`.
fn surfaces(plant: Option<&str>) -> Vec<(&'static str, Vec<String>, Vec<String>)> {
    let mut doc = read("docs/connections.md");
    let mut llms = read("www/llms.txt");
    let mut enum_ = enum_names();
    let mut tool = tool_names();
    let mut routes: Vec<String> = connect::callback_route_table()
        .iter()
        .map(|r| r.strip_prefix(connect::CALLBACK_ROUTE_PREFIX).expect("route prefix").to_string())
        .collect();
    match plant {
        Some("docs/connections.md") => doc.push_str("\n## `fakeprov`\n"),
        Some("www/llms.txt") => {
            llms = llms.replacen(connect::LLMS_SECTION_END, "- `fakeprov` (refresh: none; scopes: none)\n<!-- connections:end -->", 1)
        }
        Some("provider enum") => enum_.push("fakeprov".into()),
        Some("host.connect.providers") => tool.push("fakeprov".into()),
        Some("callback route table") => routes.push("fakeprov".into()),
        Some(other) => panic!("unknown surface {other}"),
        None => {}
    }
    vec![
        ("host.connect.providers", tool, live_names()),
        ("provider enum", enum_, live_names()),
        ("docs/connections.md", connect::names_in_doc(&doc), public_names()),
        ("www/llms.txt", connect::names_in_llms_section(&llms), public_names()),
        ("callback route table", routes, live_names()),
    ]
}

fn check(plant: Option<&str>) -> Result<(), String> {
    for (surface, names, expected) in surfaces(plant) {
        connect::check_surfaces(&expected, &[(surface, names)])?;
    }
    Ok(())
}

#[test]
fn cprov_ac02_clean_tree_passes() {
    check(None).expect("the committed surfaces agree with Provider::ALL");
}

#[test]
fn cprov_ac02_fakeprov_in_any_surface_fails_naming_it() {
    for surface in
        ["host.connect.providers", "provider enum", "docs/connections.md", "www/llms.txt", "callback route table"]
    {
        let err = check(Some(surface)).expect_err(&format!("fakeprov in {surface} must fail"));
        assert!(err.contains(surface), "error names the surface {surface}: {err}");
        assert!(err.contains("fakeprov"), "error names the extra provider: {err}");
    }
}

#[test]
fn cprov_ac02_doc_check_script_names_surface_and_extra() {
    let tmp = std::env::temp_dir().join(format!("cprov_ac02_{}", std::process::id()));
    std::fs::create_dir_all(tmp.join("docs")).unwrap();
    std::fs::create_dir_all(tmp.join("www")).unwrap();
    std::fs::write(tmp.join("providers.toml"), read("providers.toml")).unwrap();
    std::fs::write(tmp.join("docs/connections.md"), read("docs/connections.md")).unwrap();
    std::fs::write(tmp.join("www/llms.txt"), read("www/llms.txt")).unwrap();
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/connections-doc-check.sh");
    let run = || std::process::Command::new(&script).arg(&tmp).output().expect("run script");

    assert!(run().status.success(), "clean tree passes");
    let mut doc = read("docs/connections.md");
    doc.push_str("\n## `fakeprov`\n");
    std::fs::write(tmp.join("docs/connections.md"), doc).unwrap();
    let out = run();
    let _ = std::fs::remove_dir_all(&tmp);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("docs/connections.md") && stderr.contains("fakeprov"), "{stderr}");
}
