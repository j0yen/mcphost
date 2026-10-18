//! PRD-mcphost-typescript-sdk-generated -- a tool whose name is another
//! tool's dotted prefix (`host.claim.dns` beside `host.claim.dns.check`)
//! generates instead of failing. The namespace property keeps the family name
//! and the tool's own method takes a trailing underscore, the same way the
//! Python SDK names it (`client.a.b_(...)` and `client.a.b.c(...)`).

use mcphost::r#gen::sdk_ts;
use serde_json::json;

fn contract(names: &[&str]) -> Vec<u8> {
    let tools: Vec<_> = names
        .iter()
        .map(|n| {
            json!({
                "name": n,
                "description": "A fixture tool.",
                "since": "0.1.0",
                "input_schema": {"type": "object", "additionalProperties": false, "properties": {"x": {"type": "string"}}},
                "output_schema": {"type": "object", "properties": {"ok": {"type": "boolean"}}}
            })
        })
        .collect();
    serde_json::to_vec(&json!({"contract_version": 1, "kinds": [], "output_envelope_fields": [], "tools": tools})).unwrap()
}

#[test]
fn tssdk_ac08_a_tool_that_is_also_a_family_prefix_generates_and_both_verbs_are_reachable() {
    let files = sdk_ts::render(&contract(&["a.b", "a.b.c", "a.d"])).expect("a.b beside a.b.c generates");
    let client = &files["client.ts"];

    // the family keeps its namespace property and class
    assert!(client.contains("export class NsA {\n  readonly b: NsAB;\n"), "{client}");
    assert!(client.contains("this.b = new NsAB(c);"), "{client}");
    // the sub-verb is on the family, the tool itself on the parent under `b_`
    assert!(client.contains("export class NsAB {"), "{client}");
    assert!(client.contains("\n  c(args: s.ABCArgsInput): Promise<s.ABCResultOutput> {"), "{client}");
    assert!(client.contains("\n  b_(args: s.ABArgsInput): Promise<s.ABResultOutput> {"), "{client}");
    assert!(client.contains("this.#c.call(\"a.b\", s.ABArgs, s.ABResult, args)"), "{client}");
    assert!(client.contains("this.#c.call(\"a.b.c\", s.ABCArgs, s.ABCResult, args)"), "{client}");
    // an unrelated sibling keeps its plain name
    assert!(client.contains("\n  d(args: s.ADArgsInput): Promise<s.ADResultOutput> {"), "{client}");
    // schemas exist for both tools, so the client type-checks
    assert!(files["schemas.ts"].contains("export const ABArgs = ") && files["schemas.ts"].contains("export const ABCArgs = "));
}

#[test]
fn tssdk_ac08_a_tool_named_like_the_renamed_method_still_fails() {
    let err = sdk_ts::render(&contract(&["a.b", "a.b.c", "a.b_"])).unwrap_err();
    assert!(err.contains("both a tool and a family prefix"), "{err}");
}

#[test]
fn tssdk_ac08_a_contract_without_prefix_tools_has_no_underscore_methods() {
    let files = sdk_ts::render(mcphost::call_cli::EMBEDDED_CONTRACT).unwrap();
    assert!(!files["client.ts"].lines().any(|l| l.starts_with("  ") && !l.starts_with("   ") && l.contains("_(args: ")), "an existing tool was renamed");
}
