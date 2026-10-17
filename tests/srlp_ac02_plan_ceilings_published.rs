//! PRD-mcphost-sandbox-resource-limits-from-plan
//! AC2 (P0) -- Given the free and pro plans, When `ceilings_json()` is
//! called, Then both contain `sandbox_cpu_s_max`, `sandbox_memory_mb_max`,
//! `sandbox_wall_s_max`, `sandbox_pids_max` with the R1 values and
//! `ceilings_flat()` lists them.

use mcphost::plans::PlanCatalog;
use serde_json::json;

#[test]
fn srlp_ac02_ceilings_json_and_flat_carry_the_four_sandbox_fields() {
    let catalog = PlanCatalog::default_catalog();
    for (plan, cpu, mem, wall, pids) in [("free", 30, 256, 32, 16), ("pro", 60, 1024, 62, 64)] {
        let p = catalog.get(plan).expect("plan");
        let c = p.ceilings_json();
        assert_eq!(c["sandbox_cpu_s_max"], json!(cpu), "{plan}");
        assert_eq!(c["sandbox_memory_mb_max"], json!(mem), "{plan}");
        assert_eq!(c["sandbox_wall_s_max"], json!(wall), "{plan}");
        assert_eq!(c["sandbox_pids_max"], json!(pids), "{plan}");
        let flat = p.ceilings_flat();
        for (name, want) in [
            ("sandbox_cpu_s_max", cpu),
            ("sandbox_memory_mb_max", mem),
            ("sandbox_wall_s_max", wall),
            ("sandbox_pids_max", pids),
        ] {
            assert!(flat.contains(&(name.to_string(), want)), "{plan}: flat lacks {name}={want}");
        }
    }
}
