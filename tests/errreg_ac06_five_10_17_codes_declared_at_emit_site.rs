//! PRD-mcphost-error-codes-generated-registry
//! AC6 (P0) — Given `resources_max`, `sampling_calls_exceeded`,
//! `sampling_quota_exceeded`, `sampling_timeout` and `sampling_unsupported`,
//! When looked up, Then each is declared in `src/resources.rs` or
//! `src/sampling.rs` and carries Plan/PlanLimit, Plan/PlanLimit,
//! Plan/PlanLimit, Call/AgentFault retryable, Call/HostRefusal respectively.

use mcphost::error_registry::{ErrorClass, Family, lookup};

#[test]
fn errreg_ac06_five_codes_are_declared_at_their_emit_site_with_the_hand_fix_values() {
    // (code, declaring file, family, class, retryable)
    let expected = [
        ("resources_max", "src/resources.rs", Family::Plan, ErrorClass::PlanLimit, false),
        ("sampling_calls_exceeded", "src/sampling.rs", Family::Plan, ErrorClass::PlanLimit, false),
        ("sampling_quota_exceeded", "src/sampling.rs", Family::Plan, ErrorClass::PlanLimit, false),
        ("sampling_timeout", "src/sampling.rs", Family::Call, ErrorClass::AgentFault, true),
        ("sampling_unsupported", "src/sampling.rs", Family::Call, ErrorClass::HostRefusal, false),
    ];
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for (code, file, family, class, retryable) in expected {
        let e = lookup(code).unwrap_or_else(|| panic!("{code} has no registry row"));
        assert_eq!(e.family, family, "{code} family");
        assert_eq!(e.class, class, "{code} class");
        assert_eq!(e.retryable, retryable, "{code} retryable");
        assert!(e.site.starts_with(&format!("{file}:")), "{code} declared at {}", e.site);
        assert!(!e.remedy.is_empty() && e.remedy.len() <= 200, "{code} remedy length {}", e.remedy.len());
        let text = std::fs::read_to_string(root.join(file)).expect("read declaring file");
        assert!(text.contains(&format!("code: \"{code}\"")), "{code} not declared by err_code! in {file}");
    }
}
