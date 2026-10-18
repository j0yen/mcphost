//! contract: reads registry handler::host_tool_descriptors
//! PRD-mcphost-publish-visibility-and-tags AC2 (P0) -- Given
//! `host_tools()`'s descriptors, When the test reads the `visibility` enum of
//! `host.tool_publish` and of `host.tool_share`, Then each equals the slice
//! it renders from (`Visibility::PUBLISHABLE` for publish; for share,
//! `Visibility::ALL` minus what its validator rejects), and a `Visibility`
//! variant added without a schema change fails the test.

use mcphost::handler::host_tool_descriptors;
use mcphost::kinds::KindRegistry;
use mcphost::sharing::Visibility;
use serde_json::Value;

fn visibility_enum(tools: &[rmcp::model::Tool], name: &str) -> Vec<String> {
    let tool = tools.iter().find(|t| t.name == name).unwrap_or_else(|| panic!("{name} not advertised"));
    let schema = Value::Object((*tool.input_schema).clone());
    schema["properties"]["visibility"]["enum"]
        .as_array()
        .unwrap_or_else(|| panic!("{name}: visibility has no enum: {schema}"))
        .iter()
        .map(|v| v.as_str().expect("enum entry is a string").to_string())
        .collect()
}

fn names(set: &[Visibility]) -> Vec<String> {
    set.iter().map(|v| v.as_str().to_string()).collect()
}

#[test]
fn pubvis_ac02() {
    let tools = host_tool_descriptors(&KindRegistry::with_builtin());
    let publish = visibility_enum(&tools, "host.tool.publish");
    let share = visibility_enum(&tools, "host.tool.share");

    assert_eq!(publish, names(&Visibility::PUBLISHABLE), "host.tool_publish enum");

    // Every variant is either in the schema enum or refused by the
    // validator the schema describes -- never silently neither/both.
    for v in Visibility::ALL {
        let name = v.as_str().to_string();
        assert_eq!(
            publish.contains(&name),
            Visibility::parse_publishable(&name).is_ok(),
            "host.tool_publish: schema and validator disagree on '{name}'"
        );
        assert_eq!(
            share.contains(&name),
            Visibility::parse_shareable(&name).is_ok(),
            "host.tool_share: schema and validator disagree on '{name}'"
        );
        assert_eq!(Visibility::parse(&name), Some(v), "round trip for '{name}'");
    }
    // ...and the enums name nothing outside ALL.
    for name in publish.iter().chain(share.iter()) {
        assert!(Visibility::parse(name).is_some(), "enum names unknown visibility '{name}'");
    }
    assert_eq!(share, names(&Visibility::SHAREABLE), "host.tool_share enum");
    let rejected: Vec<_> = Visibility::ALL.into_iter().filter(|v| !Visibility::SHAREABLE.contains(v)).collect();
    assert_eq!(rejected, vec![Visibility::Private], "share rejects only private (unshare is the way back)");
}
