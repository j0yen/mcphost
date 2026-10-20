//! `KindDoc` (PRD-mcphost-kind-docs-from-registry R1): the one struct a
//! `docs/kinds/<kind>.md` page's generated tables are rendered from. Every
//! value is read from data the registry already holds -- the kind's
//! `known_spec_fields()` / `required_spec_field()`, [`UNIVERSAL_SPEC_FIELDS`],
//! the `PlanCatalog` ceilings, the kind's [`KindContract`] error families,
//! python's `BRIDGE_MODULES` and the kind's own `example()` -- plus two
//! `&'static str` meaning tables: [`FieldDoc`] rows beside each kind's field
//! list, and [`ERROR_WHEN`] beside the contract's error codes.

use serde_json::Value;

use super::contract::{KindContract, codes};
use super::python::BridgeModule;
use super::{KindExample, UNIVERSAL_SPEC_FIELDS};
use crate::plans::PlanCatalog;

/// One spec field's hand-written half: its JSON type and what it means.
#[derive(Debug, Clone, Copy)]
pub struct FieldDoc {
    pub name: &'static str,
    pub ty: &'static str,
    pub meaning: &'static str,
}

/// Whether a spec field must be present, as learned by probing the kind's
/// own validator (PRD-mcphost-kind-field-docs-required-from-parse-spec R1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Requiredness {
    Always,
    Never,
    /// Required unless one of these (sorted) fields is declared instead.
    Unless(Vec<&'static str>),
}

impl Requiredness {
    /// The `required` cell of a kind page's field table.
    pub fn cell(&self) -> String {
        match self {
            Requiredness::Always => "yes".to_string(),
            Requiredness::Never => "no".to_string(),
            Requiredness::Unless(fields) => format!(
                "unless {}",
                fields.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ")
            ),
        }
    }
}

fn spec_keys(spec: &Value) -> Vec<String> {
    spec.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default()
}

/// Every way `kind`'s minimal specs fail to be minimal: a spec that does not
/// validate as-is, or one that still validates with one of its own fields
/// removed. Each line names the kind, the spec index and the field.
pub fn minimality_failures<K: super::Kind + ?Sized>(kind: &K) -> Vec<String> {
    let mut out = Vec::new();
    let name = kind.name();
    let specs = kind.minimal_specs();
    if specs.is_empty() {
        out.push(format!("kind {name}: minimal_specs() is empty"));
    }
    for (i, spec) in specs.iter().enumerate() {
        if let Err(e) = kind.validate(spec) {
            out.push(format!("kind {name}: minimal spec #{i} does not parse as-is: {e}"));
            continue;
        }
        for field in spec_keys(spec) {
            let mut minus = spec.clone();
            if let Some(obj) = minus.as_object_mut() {
                obj.remove(&field);
            }
            if kind.validate(&minus).is_ok() {
                out.push(format!(
                    "kind {name}: minimal spec #{i} still parses without field `{field}`"
                ));
            }
        }
    }
    out
}

/// Probes `kind`'s validator with its [`super::Kind::minimal_specs`] to learn
/// each documented field's [`Requiredness`]. A field no minimal spec carries
/// is `Never`; one every spec carries is `Always`; otherwise it is required
/// `Unless` a field of a spec that lacks it is declared instead (fields every
/// spec shares are not alternatives). Panics, naming kind/spec/field, if a
/// minimal spec is not minimal.
pub fn derive_requiredness<K: super::Kind + ?Sized>(kind: &K) -> Vec<(FieldDoc, Requiredness)> {
    let failures = minimality_failures(kind);
    assert!(failures.is_empty(), "{}", failures.join("; "));
    let specs: Vec<Vec<String>> = kind.minimal_specs().iter().map(spec_keys).collect();
    let common: Vec<&String> = specs
        .first()
        .map(|first| first.iter().filter(|k| specs.iter().all(|s| s.contains(k))).collect())
        .unwrap_or_default();
    let docs = kind.field_docs();
    let name_of = |key: &str| docs.iter().find(|d| d.name == key).map(|d| d.name);
    kind.field_docs()
        .iter()
        .map(|doc| {
            let with: Vec<&Vec<String>> =
                specs.iter().filter(|s| s.iter().any(|k| k == doc.name)).collect();
            let req = if with.is_empty() {
                Requiredness::Never
            } else if with.len() == specs.len() {
                Requiredness::Always
            } else {
                let mut alts: Vec<&'static str> = specs
                    .iter()
                    .filter(|s| !s.iter().any(|k| k == doc.name))
                    .flatten()
                    .filter(|k| !common.contains(k))
                    .filter_map(|k| name_of(k))
                    .collect();
                alts.sort_unstable();
                alts.dedup();
                Requiredness::Unless(alts)
            };
            (*doc, req)
        })
        .collect()
}

/// [`UNIVERSAL_SPEC_FIELDS`]' meaning rows, shared by every kind.
pub const UNIVERSAL_FIELD_DOCS: &[FieldDoc] = &[
    FieldDoc {
        name: "description",
        ty: "string",
        meaning: "the tool's description in `tools/list` (every kind)",
    },
    FieldDoc {
        name: "outputs",
        ty: "array or object",
        meaning: "fields a caller can rely on in `result.payload`, optionally with a `$.a.b` path each (every kind)",
    },
    FieldDoc {
        name: "reads",
        ty: "array of string",
        meaning: "tables this tool reads, recorded for lineage and blast-radius reports",
    },
    FieldDoc {
        name: "ui",
        ty: "object",
        meaning: "optional presentation hints for clients that render a tool's result",
    },
];

/// When each error code a kind contract lists is returned.
pub const ERROR_WHEN: &[(&str, &str)] = &[
    (codes::TOOL_EXCEPTION, "the tool's code raised an uncaught exception"),
    (codes::TOOL_TRAPPED, "the tool trapped (a WebAssembly trap)"),
    (codes::TOOL_OUTPUT_INVALID, "the tool's return value did not satisfy its declared outputs"),
    (codes::TOOL_TIMEOUT, "the call ran past its `timeout_s`"),
    (codes::TOOL_OOM, "the call exceeded its `memory_mb`"),
    (codes::TOOL_PROCESS_LIMIT, "the sandboxed process hit its process or file limit"),
    (codes::TOOL_OUTPUT_TOO_LARGE, "the tool's output exceeded the result size cap"),
    (codes::TOOL_BUILDING, "the tool's dependencies are still building"),
    (codes::TOOL_NOT_FOUND, "a named tool does not exist for this tenant"),
    (codes::TOOL_RUN_UNSUPPORTED, "the tool cannot be run this way (for example a dry run)"),
    (codes::UPSTREAM_AUTH, "the upstream refused the credentials (401/403)"),
    (codes::UPSTREAM_RATE_LIMITED, "the upstream answered 429"),
    (codes::UPSTREAM_CLIENT_ERROR, "the upstream answered another 4xx"),
    (codes::UPSTREAM_ERROR, "the upstream answered 5xx"),
    (codes::UPSTREAM_TIMEOUT, "the upstream did not answer within the call's timeout"),
    (codes::UPSTREAM_UNREACHABLE, "the upstream could not be reached"),
    (codes::UPSTREAM_NOT_CONNECTED, "the named upstream provider is not connected for this tenant"),
    (codes::HOST_NOT_ALLOWED, "the target host is not on the allowed list or resolves to a private address"),
    (codes::COMPOSE_SELF_CALL, "a tool named itself as one of its own children"),
    (codes::COMPOSE_DEPTH_EXCEEDED, "composition nested deeper than 4 levels"),
    (codes::COMPOSE_CHILDREN_EXCEEDED, "one top-level call's tree exceeded 50 child calls"),
    (codes::COMPOSE_ITEM_OUTSIDE_FOR_EACH, "`$.item` or `$.index` used in a step with no `for_each`"),
    (codes::COMPOSE_MAPPING_TYPE, "a mapping resolved to the wrong type (for example a non-array `for_each`)"),
    (codes::COMPOSE_MAPPING_MISSING, "a mapping path resolved to nothing"),
    (codes::COMPOSE_FANOUT_EXCEEDED, "a `for_each` array was longer than the plan's `chain_fanout_max`"),
    (codes::COMPOSE_FANOUT_ITEM_FAILED, "an item of a `for_each` step failed"),
    (codes::COMPOSE_INPUT_MISSING, "a `$.input.<name>` mapping named an argument the call did not carry"),
];

#[derive(Debug, Clone, PartialEq)]
pub struct SpecFieldDoc {
    pub name: String,
    pub ty: String,
    pub required: Requiredness,
    pub meaning: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CeilingDoc {
    pub field: String,
    pub free: i64,
    pub pro: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ErrorDoc {
    pub code: String,
    pub when: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BridgeDoc {
    pub name: String,
    pub purpose: String,
}

/// Everything a kind page's generated blocks carry.
#[derive(Debug, Clone, PartialEq)]
pub struct KindDoc {
    pub name: String,
    pub summary: String,
    /// The kind's own `known_spec_fields()` followed by the universal ones.
    pub spec_fields: Vec<SpecFieldDoc>,
    pub universal_fields: Vec<String>,
    pub ceilings: Vec<CeilingDoc>,
    pub bridge_modules: Option<Vec<BridgeDoc>>,
    pub error_codes: Vec<ErrorDoc>,
    pub example: Value,
    /// One accepted shape each (`Kind::minimal_specs`).
    pub minimal_specs: Vec<Value>,
}

/// Assembles one kind's [`KindDoc`]; `Kind::doc`'s default body.
#[allow(clippy::too_many_arguments)]
pub fn assemble(
    name: &str,
    example: KindExample,
    known: &'static [&'static str],
    requiredness: Vec<(&'static str, Requiredness)>,
    field_docs: &'static [FieldDoc],
    ceiling_fields: &'static [&'static str],
    contract: &'static KindContract,
    bridges: Option<&'static [BridgeModule]>,
    minimal_specs: Vec<Value>,
    plans: &PlanCatalog,
) -> KindDoc {
    let spec_fields = known
        .iter()
        .chain(UNIVERSAL_SPEC_FIELDS.iter())
        .map(|field| {
            let doc = field_docs
                .iter()
                .chain(UNIVERSAL_FIELD_DOCS.iter())
                .find(|d| d.name == *field);
            SpecFieldDoc {
                name: (*field).to_string(),
                ty: doc.map(|d| d.ty).unwrap_or_default().to_string(),
                required: requiredness
                    .iter()
                    .find(|(n, _)| n == field)
                    .map(|(_, r)| r.clone())
                    .unwrap_or(Requiredness::Never),
                meaning: doc.map(|d| d.meaning).unwrap_or_default().to_string(),
            }
        })
        .collect();

    let ceiling_of = |plan: &str, field: &str| -> Option<i64> {
        plans
            .get(plan)?
            .ceilings_flat()
            .into_iter()
            .find(|(n, _)| n == field)
            .map(|(_, v)| v)
    };
    let ceilings = ceiling_fields
        .iter()
        .filter_map(|field| {
            Some(CeilingDoc {
                field: (*field).to_string(),
                free: ceiling_of("free", field)?,
                pro: ceiling_of("pro", field)?,
            })
        })
        .collect();

    let mut error_codes: Vec<ErrorDoc> = Vec::new();
    for code in contract.error_families.iter().flat_map(|f| f.codes.iter()) {
        if error_codes.iter().any(|e| e.code == *code) {
            continue;
        }
        error_codes.push(ErrorDoc {
            code: (*code).to_string(),
            when: ERROR_WHEN
                .iter()
                .find(|(c, _)| c == code)
                .map(|(_, w)| *w)
                .unwrap_or_default()
                .to_string(),
        });
    }

    KindDoc {
        name: name.to_string(),
        summary: example.blurb,
        spec_fields,
        universal_fields: UNIVERSAL_SPEC_FIELDS.iter().map(|f| (*f).to_string()).collect(),
        ceilings,
        bridge_modules: bridges.map(|mods| {
            mods.iter()
                .map(|m| BridgeDoc { name: m.name.to_string(), purpose: m.purpose.to_string() })
                .collect()
        }),
        error_codes,
        example: example.spec,
        minimal_specs,
    }
}
