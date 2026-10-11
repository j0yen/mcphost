//! PRD-mcphost-status-host-pressure AC3 (P0): a 3-field `cpu` line and a
//! `meminfo` without `MemAvailable` yield `null` for both, with no panic.

use mcphost::hostpressure::sample;

#[test]
fn short_cpu_line_and_missing_memavailable_are_null() {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-hpress-ac03-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("stat"), "cpu  100 200 300\nintr 1 2 3\n").unwrap();
    std::fs::write(dir.join("meminfo"), "MemTotal:        7936000 kB\nMemFree: 412000 kB\n").unwrap();
    std::fs::write(dir.join("loadavg"), "garbage\n").unwrap();

    let h = sample(&dir);
    assert_eq!(h.cpu_steal_pct_since_boot, None);
    assert_eq!(h.mem_available_mb, None);
    assert_eq!(h.load1, None);
    assert_eq!(h.load5, None);
    assert_eq!(h.psi_cpu_some_avg60, None);

    std::fs::write(dir.join("stat"), "\u{0}\u{1}not a stat file at all").unwrap();
    assert_eq!(sample(&dir).cpu_steal_pct_since_boot, None);
    let _ = std::fs::remove_dir_all(dir);
}
