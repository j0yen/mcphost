//! PRD-mcphost-status-host-pressure requirement 1: the host's own pressure
//! numbers, read from `/proc` on demand. Each group reads exactly one file;
//! a missing or unparsable file yields `None` (JSON `null`) for that group
//! and never an error.

use std::path::Path;

use serde::Serialize;

/// The `host` object of `GET /status.json`. `docs/metrics.md`'s field table
/// is checked against this struct's field list by
/// `scripts/host-pressure-doc-check.sh`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HostPressure {
    pub load1: Option<f64>,
    pub load5: Option<f64>,
    pub psi_cpu_some_avg60: Option<f64>,
    pub cpu_steal_pct_since_boot: Option<f64>,
    pub mem_available_mb: Option<u64>,
    pub nproc: Option<u32>,
    pub sampled_at: u64,
}

fn read(root: &Path, rel: &str) -> Option<String> {
    std::fs::read_to_string(root.join(rel)).ok()
}

fn parse_loadavg(text: &str) -> (Option<f64>, Option<f64>) {
    let mut it = text.split_whitespace();
    let load1 = it.next().and_then(|s| s.parse::<f64>().ok());
    let load5 = it.next().and_then(|s| s.parse::<f64>().ok());
    match (load1, load5) {
        (Some(a), Some(b)) => (Some(a), Some(b)),
        _ => (None, None),
    }
}

/// `some avg10=0.00 avg60=1.23 avg300=0.50 total=123` -> `1.23`.
fn parse_psi_some_avg60(text: &str) -> Option<f64> {
    let line = text.lines().find(|l| l.starts_with("some "))?;
    line.split_whitespace()
        .find_map(|tok| tok.strip_prefix("avg60="))
        .and_then(|v| v.parse::<f64>().ok())
}

/// First `cpu` line: field 8 (steal) / sum of all fields x 100.
fn parse_steal_pct(text: &str) -> Option<f64> {
    let line = text.lines().find(|l| l.split_whitespace().next() == Some("cpu"))?;
    let fields: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .map(|f| f.parse::<u64>().ok())
        .collect::<Option<_>>()?;
    let steal = *fields.get(7)?;
    let sum: u64 = fields.iter().sum();
    if sum == 0 {
        return None;
    }
    Some(steal as f64 / sum as f64 * 100.0)
}

fn parse_mem_available_mb(text: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with("MemAvailable:"))?;
    let kb = line.split_whitespace().nth(1)?.parse::<u64>().ok()?;
    Some(kb / 1024)
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Samples the host under `proc_root` (`/proc` in production).
pub fn sample(proc_root: &Path) -> HostPressure {
    let (load1, load5) = read(proc_root, "loadavg")
        .map(|t| parse_loadavg(&t))
        .unwrap_or((None, None));
    HostPressure {
        load1,
        load5,
        psi_cpu_some_avg60: read(proc_root, "pressure/cpu").and_then(|t| parse_psi_some_avg60(&t)),
        cpu_steal_pct_since_boot: read(proc_root, "stat").and_then(|t| parse_steal_pct(&t)),
        mem_available_mb: read(proc_root, "meminfo").and_then(|t| parse_mem_available_mb(&t)),
        nproc: std::thread::available_parallelism()
            .ok()
            .and_then(|n| u32::try_from(n.get()).ok()),
        sampled_at: unix_now(),
    }
}
