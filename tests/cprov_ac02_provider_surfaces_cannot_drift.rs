//! contract: reads registry handler::host_tool_descriptors
//! PRD-mcphost-connection-provider-registry AC2 / R5: the provider names on
//! each of the five surfaces equal `Provider::ALL`, and a hand edit that adds
//! `"fakeprov"` to any one of them fails naming the surface and the extra
//! name. Surfaces are read from their real sources (the live tool descriptor,
//! the committed `docs/connections.md` and `www/llms.txt`, the route table);
//! the second test injects the hand edit into a copy of each and demands the
//! drift report catch it.

use mcphost::connect::{
    ConnectProviders, Provider, all_specs, callback_routes, doc_provider_names, llms_provider_names,
    providers_json, public_provider_enum, route_provider_names, LLMS_SECTION_END,
};
use mcphost::handler::host_tool_descriptors;
use mcphost::kinds::KindRegistry;
use serde_json::{Value, json};
use std::collections::BTreeSet;

const DOC: &str = include_str!("../docs/connections.md");
const LLMS: &str = include_str!("../www/llms.txt");

/// Everything the five surfaces serve, as raw values a hand edit can touch.
#[derive(Clone)]
struct Surfaces {
    tool_rows: Vec<Value>,
    schema_enum: Vec<Value>,
    docs_md: String,
    llms_txt: String,
    routes: Vec<String>,
}

fn real_surfaces() -> Surfaces {
    let status = ConnectProviders::from_lookup(all_specs(), |_| None);
    let kinds = KindRegistry::with_builtin();
    let tool = host_tool_descriptors(&kinds)
        .into_iter()
        .find(|t| t.name == "host.connect.providers")
        .expect("host.connect.providers is registered");
    let tool = serde_json::to_value(&tool).expect("tool json");
    let schema_enum = tool["inputSchema"]["properties"]["provider"]["enum"]
        .as_array()
        .expect("provider enum")
        .clone();
    assert_eq!(schema_enum, public_provider_enum().iter().map(|n| json!(n)).collect::<Vec<_>>());
    Surfaces {
        tool_rows: providers_json(all_specs(), &status),
        schema_enum,
        docs_md: DOC.to_string(),
        llms_txt: LLMS.to_string(),
        routes: callback_routes(all_specs()),
    }
}

fn names_of(values: &[Value], key: Option<&str>) -> BTreeSet<String> {
    values
        .iter()
        .filter_map(|v| match key {
            Some(k) => v[k].as_str(),
            None => v.as_str(),
        })
        .map(str::to_string)
        .collect()
}

type HandEdit = Box<dyn Fn(&mut Surfaces)>;

/// One message per divergence, each naming the surface and the name.
fn drift_report(s: &Surfaces) -> Vec<String> {
    // The committed docs never list the test-support-only `testprov` row, so
    // it is excluded from both sides of the comparison.
    let expected: BTreeSet<String> = Provider::ALL
        .iter()
        .map(|p| p.name().to_string())
        .filter(|n| n != "testprov")
        .collect();
    let surfaces: [(&str, BTreeSet<String>); 5] = [
        ("host.connect.providers", names_of(&s.tool_rows, Some("name"))),
        ("provider enum", names_of(&s.schema_enum, None)),
        ("docs/connections.md", doc_provider_names(&s.docs_md)),
        ("www/llms.txt", llms_provider_names(&s.llms_txt)),
        ("callback route table", route_provider_names(&s.routes)),
    ];
    let mut report = Vec::new();
    for (surface, mut names) in surfaces {
        names.remove("testprov");
        for extra in names.difference(&expected) {
            report.push(format!("surface '{surface}' names '{extra}', which is not in Provider::ALL"));
        }
        for missing in expected.difference(&names) {
            report.push(format!("surface '{surface}' is missing '{missing}', which is in Provider::ALL"));
        }
    }
    report
}

#[test]
fn every_surface_names_exactly_provider_all() {
    assert_eq!(drift_report(&real_surfaces()), Vec::<String>::new());
}

#[test]
fn a_hand_added_fakeprov_on_any_one_surface_fails_naming_surface_and_name() {
    let base = real_surfaces();
    let hand_edits: [(&str, HandEdit); 5] = [
        (
            "host.connect.providers",
            Box::new(|s| s.tool_rows.push(json!({"name": "fakeprov"}))),
        ),
        ("provider enum", Box::new(|s| s.schema_enum.push(json!("fakeprov")))),
        (
            "docs/connections.md",
            Box::new(|s| s.docs_md.push_str("\n## `fakeprov`\n")),
        ),
        (
            "www/llms.txt",
            Box::new(|s| {
                s.llms_txt = s
                    .llms_txt
                    .replace(LLMS_SECTION_END, &format!("- `fakeprov` -- refresh `none`\n{LLMS_SECTION_END}"));
            }),
        ),
        (
            "callback route table",
            Box::new(|s| s.routes.push("/connect/fakeprov/callback".to_string())),
        ),
    ];
    for (surface, edit) in hand_edits {
        let mut edited = base.clone();
        edit(&mut edited);
        let report = drift_report(&edited);
        assert_eq!(report.len(), 1, "exactly the edited surface drifts: {report:?}");
        assert!(
            report[0].contains(surface) && report[0].contains("fakeprov"),
            "report must name surface '{surface}' and 'fakeprov', got: {}",
            report[0]
        );
    }
}

/// R5's CI form: `scripts/connections-doc-check.sh` agrees on the repo, and
/// fails naming the surface and the extra name on a tree with a hand edit.
#[test]
fn connections_doc_check_script_passes_on_the_repo_and_fails_on_a_hand_edit() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let script = format!("{manifest}/scripts/connections-doc-check.sh");
    let ok = std::process::Command::new("bash").arg(&script).arg(manifest).output().expect("run script");
    assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stderr));

    let tmp = std::env::temp_dir().join(format!("cprov_ac02_{}", std::process::id()));
    std::fs::create_dir_all(tmp.join("docs")).unwrap();
    std::fs::create_dir_all(tmp.join("www")).unwrap();
    std::fs::write(tmp.join("providers.toml"), include_str!("../providers.toml")).unwrap();
    std::fs::write(tmp.join("docs/connections.md"), format!("{DOC}\n## `fakeprov`\n")).unwrap();
    std::fs::write(tmp.join("www/llms.txt"), LLMS).unwrap();
    let bad = std::process::Command::new("bash").arg(&script).arg(&tmp).output().expect("run script");
    let _ = std::fs::remove_dir_all(&tmp);
    assert_eq!(bad.status.code(), Some(1));
    let err = String::from_utf8_lossy(&bad.stderr);
    assert!(err.contains("docs/connections.md") && err.contains("fakeprov"), "{err}");
}
