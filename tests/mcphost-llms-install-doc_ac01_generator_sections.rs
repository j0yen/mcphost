//! AC1 (PRD-mcphost-llms-install-doc) — Given
//! `MCPHOST_PUBLIC_URL=https://mcphost.dev`, When the generator runs, Then
//! the output has one `## ` section per install surface id from
//! `install_links::for_url`, each containing the surface's artefact in a
//! fenced block, plus a "first call" section naming `host.whoami` and
//! `onboarding.url`.

use mcphost::install_links::{self, CLIENT_INFO_NAMES, SURFACE_IDS};

const BASE: &str = "https://mcphost.dev";

/// Splits the document into `(heading, body)` pairs, one per `## ` line.
fn sections(doc: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in doc.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            out.push((h.to_string(), String::new()));
        } else if let Some((_, body)) = out.last_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    out
}

#[test]
fn one_section_per_surface_id_each_with_its_artefact_fenced() {
    let doc = install_links::render_install_doc(BASE);
    let secs = sections(&doc);
    let links = install_links::for_url(BASE);
    assert_eq!(links.len(), SURFACE_IDS.len());

    for s in &links {
        let marker = format!("(`{}`)", s.id);
        let matching: Vec<_> = secs.iter().filter(|(h, _)| h.contains(&marker)).collect();
        assert_eq!(matching.len(), 1, "exactly one `## ` section for {}: {doc}", s.id);
        let fenced = format!("```\n{}\n```", s.artefact);
        let body = &matching[0].1;
        // The opening fence may carry a language tag.
        let in_fence = body.split("```").enumerate().any(|(i, chunk)| {
            i % 2 == 1 && chunk.split_once('\n').is_some_and(|(_, c)| c.trim_end_matches('\n') == s.artefact)
        });
        assert!(in_fence, "{} artefact must sit in a fenced block ({fenced:?}): {body}", s.id);
    }
}

#[test]
fn first_call_section_names_whoami_and_onboarding_url() {
    let doc = install_links::render_install_doc(BASE);
    let secs = sections(&doc);
    let (_, body) = secs
        .iter()
        .find(|(h, _)| h.to_lowercase().contains("first call"))
        .expect("a first-call section");
    assert!(body.contains("host.whoami"), "{body}");
    assert!(body.contains("onboarding.url"), "{body}");
    assert!(body.contains("/i/<code>/mcp"), "invite URL shape: {body}");
}

#[test]
fn client_name_table_only_names_real_surface_ids() {
    let doc = install_links::render_install_doc(BASE);
    assert!(sections(&doc).iter().any(|(h, _)| h.contains("Which client")));
    for (name, id) in CLIENT_INFO_NAMES {
        assert!(SURFACE_IDS.contains(id), "clientInfo.name {name:?} maps to unknown id {id:?}");
        assert!(doc.contains(&format!("`{name}`")), "table must list {name:?}");
    }
}
