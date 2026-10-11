//! PRD-mcphost-declared-ordering-per-verb
//! AC2 (P0) -- Given `host.docs.list`, When the contract is dumped, Then its
//! `order` property enumerates exactly the `DocsOrder` variants and the
//! description ends with `default created_desc.`
//!
//! Every other `Paged` verb is held to the same shape: its `order` enum is
//! its own order enum's variants, nothing hand-typed.

use mcphost::api_contract::dump_contract;
use mcphost::kinds::KindRegistry;
use mcphost::paged::{DocsOrder, InboxOrder, OrderEnum, PAGED_VERBS, ThreadOrder, ChannelOrder, paged_spec};
use serde_json::{Value, json};

fn order_property(contract: &Value, verb: &str) -> Value {
    contract["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|t| t["name"] == json!(verb))
        .unwrap_or_else(|| panic!("{verb} missing from the contract"))["input_schema"]["properties"]["order"]
        .clone()
}

#[test]
fn docs_list_order_enumerates_exactly_the_docs_order_variants() {
    let contract = dump_contract(&KindRegistry::with_builtin());
    let order = order_property(&contract, "host.docs.list");

    let variants: Vec<Value> = DocsOrder::ALL.iter().map(|o| json!(o.name())).collect();
    assert_eq!(order["enum"], json!(variants), "enum must be exactly the DocsOrder variants: {order}");
    assert_eq!(order["enum"], json!(["created_desc", "created_asc", "name_asc"]));
    assert_eq!(order["type"], json!("string"));
    assert_eq!(order["default"], json!("created_desc"));
    let description = order["description"].as_str().expect("description");
    assert!(description.ends_with("default created_desc."), "description: {description}");
}

#[test]
fn every_paged_verb_declares_its_order_enum_from_its_order_type() {
    let contract = dump_contract(&KindRegistry::with_builtin());
    let expected: [(&str, Vec<&'static str>, &'static str); 4] = [
        ("host.docs.list", DocsOrder::names(), DocsOrder::DEFAULT.name()),
        ("host.msg.inbox", InboxOrder::names(), InboxOrder::DEFAULT.name()),
        ("host.msg.thread", ThreadOrder::names(), ThreadOrder::DEFAULT.name()),
        ("host.channel.read", ChannelOrder::names(), ChannelOrder::DEFAULT.name()),
    ];
    assert_eq!(expected.len(), PAGED_VERBS.len(), "a Paged verb was added without extending this table");
    for (verb, names, default) in expected {
        let order = order_property(&contract, verb);
        assert_eq!(order["enum"], json!(names), "{verb}: {order}");
        assert!(
            order["description"].as_str().is_some_and(|d| d.ends_with(&format!("default {default}."))),
            "{verb}: {order}"
        );
        let spec = paged_spec(verb).expect("spec");
        assert_eq!((spec.orders)(), names, "{verb}: PagedSpec disagrees with the order enum");
        assert_eq!((spec.default_order)(), default, "{verb}");
    }
}
