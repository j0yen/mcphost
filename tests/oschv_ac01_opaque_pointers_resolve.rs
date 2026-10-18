//! PRD-mcphost-output-schema-tenant-values-opaque
//! AC1 (P0) -- Given `OPAQUE_FIELDS`, When the registry loads, Then every
//! pointer resolves to an existing node that becomes `{}`, and a test with one
//! bogus pointer added proves the load fails naming it.

use mcphost::output_schemas::{OPAQUE_FIELDS, output_schema_for, widen_opaque};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

fn raw() -> BTreeMap<String, Map<String, Value>> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("contracts/host-output-schemas.v1.json");
    serde_json::from_slice(&std::fs::read(path).expect("read contract")).expect("parse contract")
}

#[test]
fn every_opaque_pointer_resolves_and_the_loaded_registry_holds_an_empty_schema_there() {
    let mut schemas = raw();
    widen_opaque(&mut schemas, OPAQUE_FIELDS).expect("every pointer resolves");
    for (tool, pointer) in OPAQUE_FIELDS {
        let loaded = output_schema_for(tool).unwrap_or_else(|| panic!("{tool} registered"));
        let node = Value::Object((*loaded).clone());
        assert_eq!(node.pointer(pointer), Some(&serde_json::json!({})), "{tool} {pointer} is not {{}} after load");
    }
}

#[test]
fn one_bogus_pointer_fails_the_widening_naming_it() {
    let mut schemas = raw();
    let mut fields = OPAQUE_FIELDS.to_vec();
    fields.push(("host.state.get", "/properties/no_such_field"));
    let err = widen_opaque(&mut schemas, &fields).expect_err("bogus pointer must fail");
    assert!(err.contains("host.state.get") && err.contains("/properties/no_such_field"), "{err}");
}
