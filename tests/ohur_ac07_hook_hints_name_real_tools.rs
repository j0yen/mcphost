//! AC7 — Given `UPSTREAM_REMEDIES` with the two own-hook rows, When every
//! `host.<x>` hint token is resolved, Then each is a live tool, including
//! `host.trigger.test` and `host.trigger.replay`.

use mcphost::handler::llms_txt_tool_names;
use mcphost::kinds::http::UPSTREAM_REMEDIES;

fn host_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("host.") {
        let tail = &rest[i..];
        let len = tail
            .char_indices()
            .find(|(_, c)| !(c.is_ascii_lowercase() || *c == '_' || *c == '.'))
            .map_or(tail.len(), |(n, _)| n);
        out.push(tail[..len].trim_end_matches('.').to_string());
        rest = &tail[len.max(1)..];
    }
    out
}

#[test]
fn ohur_ac07() {
    let names = llms_txt_tool_names(&mcphost::kinds::KindRegistry::default());
    for code in ["host_not_allowed.own_hook_event", "host_not_allowed.own_hook_inbox"] {
        let row = UPSTREAM_REMEDIES.iter().find(|r| r.code == code).unwrap_or_else(|| panic!("no row {code}"));
        let tokens = host_tokens(row.hint);
        for want in ["host.trigger.test", "host.trigger.replay"] {
            assert!(tokens.iter().any(|t| t == want), "{code} hint does not name {want}: {}", row.hint);
        }
        for t in &tokens {
            assert!(names.contains(t), "{code} hint names `{t}`, which is not a host tool");
        }
    }
}
