//! PRD-mcphost-status-host-pressure AC1 (P0): a cx33-captured `proc_root`
//! yields all six fields, and `cpu_steal_pct_since_boot` equals field 8 /
//! sum x 100 of the fixture's `cpu` line within 0.01.

use mcphost::hostpressure::sample;
use std::path::PathBuf;

const CPU_LINE: &str = "cpu  4705132 1834 1203441 98234511 20311 0 91822 713244 0 0";

fn fixture(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-hpress-ac01-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(dir.join("pressure")).expect("mkdir fixture");
    std::fs::write(dir.join("loadavg"), "0.42 0.38 0.35 2/311 48211\n").unwrap();
    std::fs::write(
        dir.join("pressure/cpu"),
        "some avg10=0.50 avg60=1.20 avg300=0.80 total=9123456\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=0\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("stat"),
        format!("{CPU_LINE}\ncpu0 1176283 458 300860 24558627 5077 0 22955 178311 0 0\nintr 1 2 3\n"),
    )
    .unwrap();
    std::fs::write(
        dir.join("meminfo"),
        "MemTotal:        7936000 kB\nMemFree:          412000 kB\nMemAvailable:    5242880 kB\nBuffers:          100000 kB\n",
    )
    .unwrap();
    dir
}

#[test]
fn cx33_fixture_yields_all_six_fields_and_steal_matches_stat() {
    let root = fixture("full");
    let h = sample(&root);
    assert_eq!(h.load1, Some(0.42));
    assert_eq!(h.load5, Some(0.38));
    assert_eq!(h.psi_cpu_some_avg60, Some(1.2));
    assert_eq!(h.mem_available_mb, Some(5120));
    assert!(h.nproc.is_some(), "nproc: {h:?}");

    let fields: Vec<f64> = CPU_LINE
        .split_whitespace()
        .skip(1)
        .map(|f| f.parse().unwrap())
        .collect();
    let expected = fields[7] / fields.iter().sum::<f64>() * 100.0;
    let got = h.cpu_steal_pct_since_boot.expect("steal Some");
    assert!((got - expected).abs() < 0.01, "got {got}, expected {expected}");
    assert!(h.sampled_at > 0);
    let _ = std::fs::remove_dir_all(root);
}
