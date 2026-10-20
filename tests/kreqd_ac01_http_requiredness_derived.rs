//! PRD-mcphost-kind-field-docs-required-from-parse-spec
//! AC1 (P0) — Given the http Kind's `field_docs()` and `minimal_specs()`,
//! When `derive_requiredness` runs, Then `method` and `url` are
//! `Unless(["upstream"])`, `upstream` is `Unless(["method","url"])`, every
//! other field is `Never`, and `FieldDoc` has no `required` member.

use mcphost::kinds::kinddoc::{FieldDoc, Requiredness, derive_requiredness};

#[test]
fn kreqd_ac01_http_requiredness_derived() {
    let registry = mcphost::kinds::production_registry();
    let kind = registry.get("http").expect("http kind registered");
    let derived = derive_requiredness(kind.as_ref());
    assert_eq!(derived.len(), kind.field_docs().len(), "one row per field_docs() entry");
    for (doc, req) in &derived {
        let expected = match doc.name {
            "method" | "url" => Requiredness::Unless(vec!["upstream"]),
            "upstream" => Requiredness::Unless(vec!["method", "url"]),
            _ => Requiredness::Never,
        };
        assert_eq!(req, &expected, "http field `{}`", doc.name);
    }
    // `FieldDoc` carries no requiredness: this destructuring lists every
    // member, so adding a `required` member breaks the build.
    let FieldDoc { name: _, ty: _, meaning: _ } = kind.field_docs()[0];
}
