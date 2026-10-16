//! PRD-mcphost-llms-full-generated-from-schemas AC3: enum values, bounds and
//! format are listed; the second level of a nested object reads `object`;
//! a schema with no `required` reads `required: none`.

use mcphost::llms_txt::{ToolView, render_sections};
use mcphost::plans::PlanCatalog;
use serde_json::json;

#[test]
fn llf_ac03() {
    let schema = json!({
        "type": "object",
        "properties": {
            "mode": {"type": "string", "enum": ["fast", "slow"]},
            "count": {"type": "integer", "minimum": 3},
            "day": {"type": "string", "format": "date"},
            "outer": {
                "type": "object",
                "properties": {
                    "inner": {"type": "object", "properties": {"leaf": {"type": "string"}}}
                }
            }
        }
    });
    let schema = schema.as_object().unwrap().clone();
    let views = [ToolView { name: "t.demo", description: "demo", schema: &schema, anonymous: false, aliases: vec![] }];
    let out = render_sections(&views, &PlanCatalog::default_catalog(), &[]);
    assert!(out.contains("| `mode` | string | no | one of: fast, slow |"), "{out}");
    assert!(out.contains("| `count` | integer | no | ≥ 3 |"), "{out}");
    assert!(out.contains("| `day` | string | no | format: date |"), "{out}");
    assert!(out.contains("| `outer` | object |"), "{out}");
    assert!(out.contains("| `outer.inner` | object |"), "{out}");
    assert!(!out.contains("`outer.inner.leaf`"), "{out}");
    assert!(out.contains("required: none\n"), "{out}");
}
