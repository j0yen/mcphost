//! PRD-mcphost-status-host-pressure AC2 (P0): without `pressure/cpu`,
//! `psi_cpu_some_avg60` is `null` and the other five fields are unchanged.

use mcphost::hostpressure::sample;
use std::path::PathBuf;

fn fixture(label: &str, with_psi: bool) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-hpress-ac02-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(dir.join("pressure")).expect("mkdir fixture");
    std::fs::write(dir.join("loadavg"), "0.42 0.38 0.35 2/311 48211\n").unwrap();
    if with_psi {
        std::fs::write(
            dir.join("pressure/cpu"),
            "some avg10=0.50 avg60=1.20 avg300=0.80 total=9123456\n",
        )
        .unwrap();
    }
    std::fs::write(dir.join("stat"), "cpu  4705132 1834 1203441 98234511 20311 0 91822 713244 0 0\n").unwrap();
    std::fs::write(dir.join("meminfo"), "MemAvailable:    5242880 kB\n").unwrap();
    dir
}

#[test]
fn removing_pressure_cpu_nulls_only_psi() {
    let (with, without) = (fixture("with", true), fixture("without", false));
    let a = sample(&with);
    let b = sample(&without);
    assert!(a.psi_cpu_some_avg60.is_some());
    assert_eq!(b.psi_cpu_some_avg60, None);
    assert_eq!(serde_json::to_value(&b).unwrap()["psi_cpu_some_avg60"], serde_json::Value::Null);
    assert_eq!(a.load1, b.load1);
    assert_eq!(a.load5, b.load5);
    assert_eq!(a.cpu_steal_pct_since_boot, b.cpu_steal_pct_since_boot);
    assert_eq!(a.mem_available_mb, b.mem_available_mb);
    assert_eq!(a.nproc, b.nproc);
    assert!(b.load1.is_some() && b.cpu_steal_pct_since_boot.is_some() && b.mem_available_mb.is_some() && b.nproc.is_some());
    let _ = std::fs::remove_dir_all(with);
    let _ = std::fs::remove_dir_all(without);
}
