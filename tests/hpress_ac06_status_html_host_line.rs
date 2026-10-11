//! PRD-mcphost-status-host-pressure AC6 (P0): `www/status.html` renders one
//! host line after the component rows, `n/a` for any null, and nothing when
//! the payload has no `host` key. The page's real inline `<script>` runs in
//! `boa_engine` against a mocked DOM and `fetch` (same harness as
//! `statusfeed_ac08_status_html_renders`); `scripts/www-check.sh` must also
//! pass over the edited page.

use boa_engine::{Context, Source};

fn read_status_html() -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("www/status.html"))
        .expect("read www/status.html")
}

/// Returns `#status-components`'s innerHTML after the page script has
/// consumed `payload` as `/status.json`'s body.
fn components_html(payload: &serde_json::Value) -> String {
    let html = read_status_html();
    let open = html.find("<script>").expect("script tag") + "<script>".len();
    let close = html[open..].find("</script>").map(|i| open + i).expect("script close");
    let mut context = Context::default();
    let dom = r#"
        var __els = {};
        ["status-fallback", "status-live", "status-state", "status-components", "status-incidents"]
            .forEach(function (id) { __els[id] = { id: id, hidden: false, textContent: "", innerHTML: "" }; });
        var document = { getElementById: function (id) { return __els[id]; } };
    "#;
    let fetch = format!(
        "function fetch() {{ return Promise.resolve({{ ok: true, status: 200, json: function () {{ return Promise.resolve({payload}); }} }}); }}"
    );
    context.eval(Source::from_bytes(dom)).expect("dom");
    context.eval(Source::from_bytes(fetch.as_str())).expect("fetch");
    context.eval(Source::from_bytes(&html[open..close])).expect("page script");
    context.run_jobs().expect("microtasks");
    context
        .eval(Source::from_bytes(r#"__els["status-components"].innerHTML"#))
        .expect("readback")
        .as_string()
        .expect("string")
        .to_std_string_escaped()
}

fn base(host: Option<serde_json::Value>) -> serde_json::Value {
    let mut v = serde_json::json!({
        "state": "operational",
        "generated_at": 1_700_000_000i64,
        "components": [{"name": "mcp", "state": "ok", "uptime_90d": 0.9999}],
        "incidents_open": [],
        "incidents_recent_30d": []
    });
    if let Some(h) = host {
        v["host"] = h;
    }
    v
}

#[test]
fn null_load1_renders_na_and_the_line_follows_the_components() {
    let out = components_html(&base(Some(serde_json::json!({
        "load1": null, "load5": 0.38, "psi_cpu_some_avg60": 1.2,
        "cpu_steal_pct_since_boot": null, "mem_available_mb": 5120, "nproc": 4,
        "sampled_at": 1_700_000_000u64
    }))));
    assert!(
        out.contains("host  load n/a/0.38  psi60 1.2%  steal n/a  mem 5120 MB  4 cpu"),
        "{out}"
    );
    assert!(out.find("mcp").unwrap() < out.find("host  load").unwrap(), "host line after rows: {out}");
}

#[test]
fn payload_without_host_renders_no_host_line() {
    let out = components_html(&base(None));
    assert!(out.contains("mcp"), "{out}");
    assert!(!out.contains("host  load"), "{out}");
}

#[test]
fn www_check_lane_passes() {
    let out = std::process::Command::new("bash")
        .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/www-check.sh"))
        .output()
        .expect("run www-check.sh");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}
